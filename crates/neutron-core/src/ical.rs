//! Minimal iCalendar (RFC 5545) reader and writer for the agenda and for
//! iMIP invitations (RFC 6047 / RFC 5546).
//!
//! Supported: VEVENT with UID, SUMMARY, LOCATION, DESCRIPTION, DTSTART/DTEND
//! (UTC, TZID via the IANA database or the VTIMEZONE's standard offset,
//! floating, all-day), DURATION, ORGANIZER, ATTENDEE (CN, PARTSTAT),
//! SEQUENCE, STATUS, RRULE (FREQ/INTERVAL/COUNT/UNTIL, BYDAY for weekly)
//! and RECURRENCE-ID. Everything here is parsing of untrusted input: sizes
//! are bounded and nothing is ever executed or fetched.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;

use crate::{Error, Result};

/// Calendar data larger than this is ignored.
pub const MAX_ICS_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 2000;
const MAX_OCCURRENCES: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Prop {
    pub fn param(&self, k: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Attendee {
    pub email: String,
    pub name: String,
    /// NEEDS-ACTION, ACCEPTED, TENTATIVE, DECLINED, DELEGATED
    pub partstat: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    /// Unix time (UTC). All-day events start at local midnight.
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub organizer_email: String,
    pub organizer_name: String,
    pub attendees: Vec<Attendee>,
    pub sequence: u32,
    pub status: String,
    pub rrule: Option<String>,
    /// Raw RECURRENCE-ID value line (a single occurrence of a series).
    #[serde(skip)]
    pub recurrence_id: Option<Prop>,
}

/// Unfold (RFC 5545 §3.1) and split into content lines.
pub fn content_lines(ics: &str) -> Vec<Prop> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let l = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = l.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !l.is_empty() {
            lines.push(l.to_string());
        }
    }
    lines.iter().filter_map(|l| parse_line(l)).collect()
}

fn parse_line(l: &str) -> Option<Prop> {
    // name *(";" param) ":" value; ':' and ';' inside quoted params don't count.
    let mut in_q = false;
    let mut colon = None;
    for (i, c) in l.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&l[..colon], &l[colon + 1..]);
    let mut parts = Vec::new();
    let mut cur = String::new();
    in_q = false;
    for c in head.chars() {
        match c {
            '"' => in_q = !in_q,
            ';' if !in_q => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.first()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts[1..]
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn escape_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
        .replace('\r', "")
}

fn mailto(v: &str) -> String {
    let v = v.trim();
    let e = if v.len() > 7 && v[..7].eq_ignore_ascii_case("mailto:") {
        &v[7..]
    } else {
        v
    };
    e.trim().to_string()
}

/// A VTIMEZONE's standard-time UTC offset in seconds (approximation used
/// only when the TZID is not an IANA name).
fn vtimezone_offsets(props: &[Prop]) -> Vec<(String, i32)> {
    let mut out = Vec::new();
    let mut tzid = None;
    let mut in_std = false;
    let mut depth_tz = false;
    for p in props {
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VTIMEZONE") => {
                depth_tz = true;
                tzid = None;
            }
            ("END", "VTIMEZONE") => depth_tz = false,
            ("BEGIN", "STANDARD") if depth_tz => in_std = true,
            ("END", "STANDARD") => in_std = false,
            ("TZID", _) if depth_tz => tzid = Some(p.value.clone()),
            ("TZOFFSETTO", _) if in_std => {
                if let (Some(id), Some(off)) = (tzid.clone(), parse_offset(&p.value)) {
                    if !out.iter().any(|(i, _): &(String, i32)| *i == id) {
                        out.push((id, off));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn parse_offset(v: &str) -> Option<i32> {
    let v = v.trim();
    let sign = match v.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let d = &v[1..];
    if d.len() < 4 || !d.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = d[0..2].parse().ok()?;
    let m: i32 = d[2..4].parse().ok()?;
    let s: i32 = d.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
    Some(sign * (h * 3600 + m * 60 + s))
}

fn iana(tzid: &str) -> Option<chrono_tz::Tz> {
    let t = tzid.trim().trim_matches('"');
    if let Ok(tz) = t.parse() {
        return Some(tz);
    }
    // EDS/libical style "/freeassociation.sourceforge.net/Europe/Istanbul".
    let segs: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    (1..segs.len()).find_map(|i| segs[i..].join("/").parse().ok())
}

/// Parse a DATE or DATE-TIME property to (unix time, all_day).
pub fn parse_time(p: &Prop, tzs: &[(String, i32)]) -> Option<(i64, bool)> {
    let v = p.value.trim();
    let is_date = p
        .param("VALUE")
        .is_some_and(|x| x.eq_ignore_ascii_case("DATE"))
        || (v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()));
    if is_date {
        let d = NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d").ok()?;
        let local = chrono::Local
            .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
            .earliest()?;
        return Some((local.timestamp(), true));
    }
    if let Some(u) = v.strip_suffix(['Z', 'z']) {
        let n = NaiveDateTime::parse_from_str(u, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n).timestamp(), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    if let Some(tzid) = p.param("TZID") {
        if let Some(tz) = iana(tzid) {
            return Some((tz.from_local_datetime(&n).earliest()?.timestamp(), false));
        }
        if let Some((_, off)) = tzs.iter().find(|(id, _)| id == tzid.trim_matches('"')) {
            return Some((n.and_utc().timestamp() - i64::from(*off), false));
        }
    }
    // Floating time: the user's local time.
    Some((
        chrono::Local
            .from_local_datetime(&n)
            .earliest()?
            .timestamp(),
        false,
    ))
}

/// RFC 5545 DURATION (e.g. PT1H30M, P1D, -PT15M) in seconds.
pub fn parse_duration(v: &str) -> Option<i64> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut time = false;
    for c in v.chars() {
        match c {
            'T' => time = true,
            '0'..='9' => num.push(c),
            u => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (u, time) {
                    ('W', _) => 7 * 86_400,
                    ('D', _) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

#[derive(Debug, Clone, Default)]
pub struct Calendar {
    pub method: String,
    pub events: Vec<Event>,
}

/// Parse a VCALENDAR (or a bare VEVENT, as EDS returns them).
pub fn parse(ics: &str) -> Result<Calendar> {
    if ics.len() > MAX_ICS_BYTES {
        return Err(Error::Invalid("calendar data too large".into()));
    }
    let props = content_lines(ics);
    let tzs = vtimezone_offsets(&props);
    let mut cal = Calendar::default();
    let mut cur: Option<(Event, Option<i64>)> = None;
    let mut depth_other = 0usize;
    for p in &props {
        let val_up = p.value.trim().to_ascii_uppercase();
        match p.name.as_str() {
            "BEGIN" if val_up == "VEVENT" && cur.is_none() => {
                cur = Some((Event::default(), None));
                continue;
            }
            "END" if val_up == "VEVENT" => {
                if let Some((mut e, dur)) = cur.take() {
                    if e.end <= e.start {
                        e.end = match dur {
                            Some(d) => e.start + d,
                            None if e.all_day => e.start + 86_400,
                            None => e.start,
                        };
                    }
                    if !e.uid.is_empty() && e.start != 0 && cal.events.len() < MAX_EVENTS {
                        cal.events.push(e);
                    }
                }
                continue;
            }
            "BEGIN" if cur.is_some() => depth_other += 1, // VALARM etc.
            "END" if cur.is_some() && depth_other > 0 => depth_other -= 1,
            "METHOD" if cur.is_none() => cal.method = val_up.clone(),
            _ => {}
        }
        let Some((e, dur)) = cur.as_mut() else {
            continue;
        };
        if depth_other > 0 {
            continue;
        }
        match p.name.as_str() {
            "UID" => e.uid = p.value.trim().chars().take(500).collect(),
            "SUMMARY" => e.summary = unescape_text(&p.value).chars().take(500).collect(),
            "LOCATION" => e.location = unescape_text(&p.value).chars().take(500).collect(),
            "DESCRIPTION" => e.description = unescape_text(&p.value).chars().take(4000).collect(),
            "DTSTART" => {
                if let Some((t, d)) = parse_time(p, &tzs) {
                    e.start = t;
                    e.all_day = d;
                }
            }
            "DTEND" => {
                if let Some((t, _)) = parse_time(p, &tzs) {
                    e.end = t;
                }
            }
            "DURATION" => *dur = parse_duration(&p.value),
            "SEQUENCE" => e.sequence = p.value.trim().parse().unwrap_or(0),
            "STATUS" => e.status = val_up.clone(),
            "RRULE" => e.rrule = Some(p.value.trim().to_string()),
            "RECURRENCE-ID" => e.recurrence_id = Some(p.clone()),
            "ORGANIZER" => {
                e.organizer_email = mailto(&p.value);
                e.organizer_name = p.param("CN").unwrap_or("").trim_matches('"').to_string();
            }
            "ATTENDEE" if e.attendees.len() < 500 => e.attendees.push(Attendee {
                email: mailto(&p.value),
                name: p.param("CN").unwrap_or("").trim_matches('"').to_string(),
                partstat: p
                    .param("PARTSTAT")
                    .unwrap_or("NEEDS-ACTION")
                    .to_ascii_uppercase(),
            }),
            _ => {}
        }
    }
    Ok(cal)
}

/// Occurrences `(start, end)` of `e` overlapping `[from, to)`.
pub fn occurrences(e: &Event, from: i64, to: i64) -> Vec<(i64, i64)> {
    let len = (e.end - e.start).max(0);
    let Some(rule) = e.rrule.as_deref() else {
        return if e.start < to && e.start + len.max(1) > from {
            vec![(e.start, e.start + len)]
        } else {
            vec![]
        };
    };
    let mut freq = "";
    let mut interval = 1i64;
    let mut count: Option<usize> = None;
    let mut until: Option<i64> = None;
    let mut byday: Vec<chrono::Weekday> = Vec::new();
    for kv in rule.split(';') {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => freq = v,
            "INTERVAL" => interval = v.parse::<i64>().unwrap_or(1).max(1),
            "COUNT" => count = v.parse().ok(),
            "UNTIL" => {
                until = parse_time(
                    &Prop {
                        name: "UNTIL".into(),
                        params: vec![],
                        value: v.into(),
                    },
                    &[],
                )
                .map(|(t, d)| if d { t + 86_399 } else { t })
            }
            "BYDAY" => {
                byday = v
                    .split(',')
                    .filter_map(|d| {
                        let d = d.trim_start_matches(|c: char| {
                            c == '-' || c == '+' || c.is_ascii_digit()
                        });
                        Some(match d.to_ascii_uppercase().as_str() {
                            "MO" => chrono::Weekday::Mon,
                            "TU" => chrono::Weekday::Tue,
                            "WE" => chrono::Weekday::Wed,
                            "TH" => chrono::Weekday::Thu,
                            "FR" => chrono::Weekday::Fri,
                            "SA" => chrono::Weekday::Sat,
                            "SU" => chrono::Weekday::Sun,
                            _ => return None,
                        })
                    })
                    .collect()
            }
            _ => {}
        }
    }
    let Some(start) = DateTime::<Utc>::from_timestamp(e.start, 0) else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut emitted = 0usize;
    let mut push = |t: DateTime<Utc>, out: &mut Vec<(i64, i64)>| -> bool {
        let s = t.timestamp();
        if until.is_some_and(|u| s > u) || count.is_some_and(|c| emitted >= c) || s >= to {
            return false;
        }
        emitted += 1;
        if s + len.max(1) > from {
            out.push((s, s + len));
        }
        out.len() < MAX_OCCURRENCES
    };
    let step = |i: i64| -> Option<DateTime<Utc>> {
        match freq.to_ascii_uppercase().as_str() {
            "DAILY" => Some(start + Duration::days(i * interval)),
            "WEEKLY" => Some(start + Duration::weeks(i * interval)),
            "MONTHLY" => start.checked_add_months(chrono::Months::new((i * interval) as u32)),
            "YEARLY" => start.checked_add_months(chrono::Months::new((i * interval * 12) as u32)),
            _ => None,
        }
    };
    let weekly_byday = freq.eq_ignore_ascii_case("WEEKLY") && !byday.is_empty();
    for i in 0..100_000i64 {
        let Some(base) = step(i) else { break };
        if base.timestamp() >= to && !weekly_byday {
            break;
        }
        if weekly_byday {
            // The week (Mon..Sun) containing `base`.
            let monday = base - Duration::days(i64::from(base.weekday().num_days_from_monday()));
            if monday.timestamp() >= to {
                break;
            }
            let mut days: Vec<DateTime<Utc>> = byday
                .iter()
                .map(|wd| monday + Duration::days(i64::from(wd.num_days_from_monday())))
                .filter(|d| *d >= start)
                .collect();
            days.sort();
            for d in days {
                if !push(d, &mut out) {
                    return out;
                }
            }
        } else if !push(base, &mut out) {
            return out;
        }
    }
    out
}

/// What the invite card in the reading pane shows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InviteView {
    /// REQUEST, CANCEL, REPLY, PUBLISH, …
    pub method: String,
    pub event: Event,
    /// The user's PARTSTAT (recorded answer, else what the invite says).
    pub my_partstat: String,
    pub responded: bool,
    pub in_calendar: bool,
    /// Accept/Tentative/Decline makes sense (a REQUEST with an organizer
    /// that is not the user).
    pub can_respond: bool,
}

pub fn invite_view(
    ics: &str,
    me_email: &str,
    recorded: Option<(String, bool)>,
) -> Option<InviteView> {
    let cal = parse(ics).ok()?;
    let event = cal.events.into_iter().next()?;
    let method = if cal.method.is_empty() {
        "PUBLISH".to_string()
    } else {
        cal.method
    };
    let listed = event
        .attendees
        .iter()
        .find(|a| a.email.eq_ignore_ascii_case(me_email))
        .map(|a| a.partstat.clone());
    let (my_partstat, responded, in_calendar) = match recorded {
        Some((p, c)) => (p, true, c),
        None => (
            listed.unwrap_or_else(|| "NEEDS-ACTION".into()),
            false,
            false,
        ),
    };
    let can_respond = method == "REQUEST"
        && !event.organizer_email.is_empty()
        && !event.organizer_email.eq_ignore_ascii_case(me_email);
    Some(InviteView {
        method,
        event,
        my_partstat,
        responded,
        in_calendar,
        can_respond,
    })
}

fn fold(line: &str) -> String {
    // 75 octets per line, never splitting a UTF-8 character.
    let mut out = String::new();
    let mut n = 0;
    for c in line.chars() {
        let l = c.len_utf8();
        if n + l > 75 {
            out.push_str("\r\n ");
            n = 1;
        }
        out.push(c);
        n += l;
    }
    out.push_str("\r\n");
    out
}

fn utc_stamp(t: i64) -> String {
    DateTime::<Utc>::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

fn date_stamp(t: i64) -> String {
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .unwrap_or_default()
        .format("%Y%m%d")
        .to_string()
}

fn time_lines(e: &Event, out: &mut String) {
    if e.all_day {
        out.push_str(&fold(&format!(
            "DTSTART;VALUE=DATE:{}",
            date_stamp(e.start)
        )));
        out.push_str(&fold(&format!("DTEND;VALUE=DATE:{}", date_stamp(e.end))));
    } else {
        out.push_str(&fold(&format!("DTSTART:{}", utc_stamp(e.start))));
        out.push_str(&fold(&format!("DTEND:{}", utc_stamp(e.end))));
    }
}

fn quote_param(v: &str) -> String {
    let v: String = v.chars().filter(|c| *c != '"' && !c.is_control()).collect();
    format!("\"{v}\"")
}

fn safe_addr(e: &str) -> Result<String> {
    let e = e.trim();
    if e.is_empty()
        || e.len() > 254
        || !e.contains('@')
        || e.chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == ':' || c == ';' || c == '"')
    {
        return Err(Error::Invalid(format!("bad calendar address {e:?}")));
    }
    Ok(e.to_string())
}

pub fn valid_partstat(p: &str) -> Result<&'static str> {
    match p.to_ascii_uppercase().as_str() {
        "ACCEPTED" => Ok("ACCEPTED"),
        "TENTATIVE" => Ok("TENTATIVE"),
        "DECLINED" => Ok("DECLINED"),
        _ => Err(Error::Invalid(format!("unknown response {p:?}"))),
    }
}

/// iTIP REPLY (RFC 5546 §3.2.3) for attendee `me` answering `e`.
pub fn build_reply(
    e: &Event,
    me_email: &str,
    me_name: &str,
    partstat: &str,
    now: i64,
) -> Result<String> {
    let ps = valid_partstat(partstat)?;
    let me = safe_addr(me_email)?;
    let org = safe_addr(&e.organizer_email)?;
    let mut s = String::from("BEGIN:VCALENDAR\r\nPRODID:-//Neutron Mail//Neutron Mail//EN\r\nVERSION:2.0\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\n");
    s.push_str(&fold(&format!("UID:{}", e.uid.replace(['\r', '\n'], ""))));
    s.push_str(&fold(&format!("SEQUENCE:{}", e.sequence)));
    s.push_str(&fold(&format!("DTSTAMP:{}", utc_stamp(now))));
    if let Some(r) = &e.recurrence_id {
        let mut line = "RECURRENCE-ID".to_string();
        for (k, v) in &r.params {
            line.push_str(&format!(";{k}={}", v.replace(['\r', '\n', ':'], "")));
        }
        line.push_str(&format!(":{}", r.value.replace(['\r', '\n'], "")));
        s.push_str(&fold(&line));
    }
    time_lines(e, &mut s);
    s.push_str(&fold(&format!("SUMMARY:{}", escape_text(&e.summary))));
    let org_cn = if e.organizer_name.is_empty() {
        String::new()
    } else {
        format!(";CN={}", quote_param(&e.organizer_name))
    };
    s.push_str(&fold(&format!("ORGANIZER{org_cn}:mailto:{org}")));
    let cn = if me_name.is_empty() {
        String::new()
    } else {
        format!(";CN={}", quote_param(me_name))
    };
    s.push_str(&fold(&format!("ATTENDEE;PARTSTAT={ps}{cn}:mailto:{me}")));
    s.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
    Ok(s)
}

/// A standalone VEVENT for the user's own calendar (times in UTC, with
/// the user's participation status).
pub fn build_calendar_event(e: &Event, me_email: &str, partstat: &str, now: i64) -> Result<String> {
    let ps = valid_partstat(partstat)?;
    let me = safe_addr(me_email)?;
    let mut s = String::from("BEGIN:VEVENT\r\n");
    s.push_str(&fold(&format!("UID:{}", e.uid.replace(['\r', '\n'], ""))));
    s.push_str(&fold(&format!("SEQUENCE:{}", e.sequence)));
    s.push_str(&fold(&format!("DTSTAMP:{}", utc_stamp(now))));
    time_lines(e, &mut s);
    if let Some(r) = e.rrule.as_deref() {
        s.push_str(&fold(&format!(
            "RRULE:{}",
            r.replace(['\r', '\n', ':'], "")
        )));
    }
    s.push_str(&fold(&format!("SUMMARY:{}", escape_text(&e.summary))));
    if !e.location.is_empty() {
        s.push_str(&fold(&format!("LOCATION:{}", escape_text(&e.location))));
    }
    if !e.description.is_empty() {
        s.push_str(&fold(&format!(
            "DESCRIPTION:{}",
            escape_text(&e.description)
        )));
    }
    if ps == "TENTATIVE" {
        s.push_str("STATUS:TENTATIVE\r\n");
    } else {
        s.push_str("STATUS:CONFIRMED\r\n");
    }
    if let Ok(org) = safe_addr(&e.organizer_email) {
        s.push_str(&fold(&format!("ORGANIZER:mailto:{org}")));
    }
    for a in &e.attendees {
        if a.email.eq_ignore_ascii_case(&me) {
            continue;
        }
        if let Ok(addr) = safe_addr(&a.email) {
            s.push_str(&fold(&format!(
                "ATTENDEE;PARTSTAT={}:mailto:{addr}",
                match a.partstat.as_str() {
                    "ACCEPTED" | "TENTATIVE" | "DECLINED" | "DELEGATED" => a.partstat.as_str(),
                    _ => "NEEDS-ACTION",
                }
            )));
        }
    }
    s.push_str(&fold(&format!("ATTENDEE;PARTSTAT={ps}:mailto:{me}")));
    s.push_str("END:VEVENT\r\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Test//EN\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\n\
BEGIN:VTIMEZONE\r\nTZID:Custom Zone\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0300\r\nTZOFFSETTO:+0300\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:abc-123@example.test\r\nSEQUENCE:2\r\nDTSTAMP:20261001T000000Z\r\n\
DTSTART;TZID=Europe/Istanbul:20261020T140000\r\nDTEND;TZID=\"Custom Zone\":20261020T150000\r\n\
SUMMARY:Design review\\, Q4\r\nLOCATION:Room 4\r\nDESCRIPTION:Line one\\nLine two that is long enough to be\r\n  folded\r\n\
ORGANIZER;CN=\"Elena Marsh\":mailto:elena@example.test\r\n\
ATTENDEE;CN=Kaan Durmus;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:kaan@localhost\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:MAILTO:bob@example.test\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_invite() {
        let c = parse(INVITE).unwrap();
        assert_eq!(c.method, "REQUEST");
        let e = &c.events[0];
        assert_eq!(e.uid, "abc-123@example.test");
        assert_eq!(e.summary, "Design review, Q4");
        assert_eq!(
            e.description,
            "Line one\nLine two that is long enough to be folded"
        );
        // 14:00 Istanbul (+03) = 11:00Z; 15:00 at a +03 VTIMEZONE = 12:00Z
        assert_eq!(e.start, 1_792_494_000);
        assert_eq!(e.end - e.start, 3600);
        assert_eq!(e.sequence, 2);
        assert_eq!(e.organizer_email, "elena@example.test");
        assert_eq!(e.organizer_name, "Elena Marsh");
        assert_eq!(e.attendees.len(), 2);
        assert_eq!(e.attendees[1].email, "bob@example.test");
        assert_eq!(e.attendees[0].partstat, "NEEDS-ACTION");
    }

    #[test]
    fn reply_is_rfc5546_shaped_and_injection_safe() {
        let mut e = parse(INVITE).unwrap().events.remove(0);
        e.summary = "Evil\r\nATTENDEE:mailto:x@y".into();
        let r = build_reply(&e, "kaan@localhost", "Kaan \"K\" Durmus", "accepted", 0).unwrap();
        assert!(r.contains("METHOD:REPLY\r\n"));
        assert!(
            r.contains("ATTENDEE;PARTSTAT=ACCEPTED;CN=\"Kaan K Durmus\":mailto:kaan@localhost\r\n")
        );
        assert!(r.contains("SEQUENCE:2\r\n"));
        assert!(r.contains("SUMMARY:Evil\\nATTENDEE:mailto:x@y\r\n"));
        assert_eq!(r.matches("ATTENDEE").count(), 2); // ours + the escaped text
        assert!(r.lines().all(|l| l.len() <= 76));
        let back = parse(&r).unwrap();
        assert_eq!(back.events[0].attendees.len(), 1);
        assert_eq!(back.events[0].start, e.start);
        assert!(build_reply(&e, "kaan@localhost", "", "maybe", 0).is_err());
        assert!(build_reply(&e, "a:b@c", "", "DECLINED", 0).is_err());
    }

    #[test]
    fn calendar_event_round_trip() {
        let e = parse(INVITE).unwrap().events.remove(0);
        let v = build_calendar_event(&e, "kaan@localhost", "TENTATIVE", 0).unwrap();
        let back = parse(&v).unwrap().events.remove(0);
        assert_eq!((back.start, back.end), (e.start, e.end));
        assert_eq!(back.status, "TENTATIVE");
        assert!(back
            .attendees
            .iter()
            .any(|a| a.email == "kaan@localhost" && a.partstat == "TENTATIVE"));
    }

    #[test]
    fn durations_dates_and_tz_paths() {
        assert_eq!(parse_duration("PT1H30M"), Some(5400));
        assert_eq!(parse_duration("P1W"), Some(604_800));
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        let p = Prop {
            name: "DTSTART".into(),
            params: vec![(
                "TZID".into(),
                "/freeassociation.sourceforge.net/Europe/Berlin".into(),
            )],
            value: "20260115T090000".into(),
        };
        assert_eq!(parse_time(&p, &[]).unwrap().0, 1_768_464_000); // 08:00Z in winter
        let ics = "BEGIN:VEVENT\nUID:x\nDTSTART;VALUE=DATE:20261020\nEND:VEVENT\n";
        let e = parse(ics).unwrap().events.remove(0);
        assert!(e.all_day);
        assert_eq!(e.end - e.start, 86_400);
    }

    #[test]
    fn recurrence_expansion() {
        let base = 1_792_494_000; // Tue 2026-10-20 11:00Z
        let mut e = Event {
            uid: "r".into(),
            start: base,
            end: base + 1800,
            rrule: Some("FREQ=DAILY;COUNT=5".into()),
            ..Default::default()
        };
        let o = occurrences(&e, base + 86_400, base + 30 * 86_400);
        assert_eq!(o.len(), 4);
        e.rrule = Some("FREQ=WEEKLY;BYDAY=MO,TU;UNTIL=20261103T235959Z".into());
        let o = occurrences(&e, base, base + 60 * 86_400);
        // Tue 20, Mon 26, Tue 27, Mon 2, Tue 3
        assert_eq!(o.len(), 5);
        assert!(occurrences(
            &Event {
                rrule: None,
                ..e.clone()
            },
            base + 3600,
            base + 7200
        )
        .is_empty());
    }

    #[test]
    fn rejects_huge_input() {
        assert!(parse(&"X".repeat(MAX_ICS_BYTES + 1)).is_err());
    }
}
