//! Module for fetching and processing CalDAV events from Infomaniak

use base64::Engine;
use chrono::{Datelike, DateTime, Duration, Local, NaiveDate, NaiveDateTime, Timelike, Utc};
use ical::parser::ical::component::IcalEvent;
use ical::parser::Component;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;

// --- Public Models ---

#[derive(Debug, Serialize, Clone)]
pub struct Event {
    pub summary: String,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct CalendarResponse {
    pub events_by_date: HashMap<String, Vec<Event>>,
}

// --- CalDAV Configuration ---

pub struct CalDavConfig {
    pub url: String,
    pub calendar_uri: String,
    pub login: String,
    pub password: String,
}

impl CalDavConfig {
    pub fn from_env() -> Self {
        CalDavConfig {
            url: std::env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".into()),
            calendar_uri: std::env::var("CALDAV_CALENDAR_URI").expect("CALDAV_CALENDAR_URI doit etre defini"),
            login: std::env::var("CALDAV_LOGIN").expect("CALDAV_LOGIN doit etre defini"),
            password: std::env::var("CALDAV_PASSWORD").expect("CALDAV_PASSWORD doit etre defini"),
        }
    }

    pub fn calendar_url(&self) -> String {
        format!("{}{}", self.url.trim_end_matches('/'), self.calendar_uri)
    }
}

// --- Date Helpers ---

pub fn get_sliding_week_range() -> (NaiveDate, NaiveDate) {
    let today = Local::now().date_naive();
    (today, today + Duration::days(7))
}

fn format_display_date(dt: &NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

// --- ICS Parsing with recurrence expansion ---

fn extract_rrule(event: &IcalEvent) -> Option<String> {
    event.get_property("RRULE").and_then(|p| p.value.clone())
}

fn parse_ical_datetime(s: &str) -> Result<NaiveDateTime, ()> {
    let cleaned = s.trim_end_matches('Z');
    if cleaned.len() == 15 {
        NaiveDateTime::parse_from_str(cleaned, "%Y%m%dT%H%M%S").map_err(|_| ())
    } else if cleaned.len() == 8 {
        NaiveDate::parse_from_str(cleaned, "%Y%m%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .map_err(|_| ())
    } else {
        Err(())
    }
}

fn expand_recurring_event(
    summary: &str,
    start: NaiveDateTime,
    end: NaiveDateTime,
    rrule: &str,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Vec<(String, NaiveDateTime, NaiveDateTime)> {
    let mut occurrences = Vec::new();
    let dur = end - start;
    let start_date = start.date();

    if start_date >= range_start && start_date <= range_end {
        occurrences.push((summary.to_string(), start, end));
    }

    let freq = rrule.split(';')
        .find(|part| part.starts_with("FREQ="))
        .and_then(|f| f.split('=').nth(1))
        .map(|f| f.to_uppercase());

    let until = rrule.split(';')
        .find(|part| part.starts_with("UNTIL="))
        .and_then(|u| u.split('=').nth(1))
        .and_then(|u| {
            let cleaned = u.trim_end_matches('Z');
            if cleaned.len() == 15 {
                NaiveDateTime::parse_from_str(cleaned, "%Y%m%dT%H%M%S").ok().map(|dt| dt.date())
            } else if cleaned.len() == 8 {
                NaiveDate::parse_from_str(cleaned, "%Y%m%d").ok()
            } else {
                None
            }
        });

    let count: Option<usize> = rrule.split(';')
        .find(|part| part.starts_with("COUNT="))
        .and_then(|c| c.split('=').nth(1))
        .and_then(|c| c.parse().ok());

    let interval: u32 = rrule.split(';')
        .find(|part| part.starts_with("INTERVAL="))
        .and_then(|i| i.split('=').nth(1))
        .and_then(|i| i.parse().ok())
        .unwrap_or(1);

    if let Some(freq) = freq {
        let mut current_date = start_date;
        let mut iteration_count = 0;

        loop {
            if iteration_count >= 500 {
                break;
            }
            iteration_count += 1;

            current_date = match freq.as_str() {
                "DAILY" => current_date + Duration::days(interval as i64),
                "WEEKLY" => current_date + Duration::weeks(interval as i64),
                "MONTHLY" => {
                    let mut y = current_date.year();
                    let mut m = current_date.month();
                    m += interval as u32;
                    while m > 12 {
                        m -= 12;
                        y += 1;
                    }
                    let day = current_date.day().min(28);
                    NaiveDate::from_ymd_opt(y, m, day).unwrap_or(current_date)
                }
                "YEARLY" => {
                    let y = current_date.year() + interval as i32;
                    NaiveDate::from_ymd_opt(y, current_date.month(), current_date.day()).unwrap_or(current_date)
                }
                _ => break,
            };

            if current_date > range_end {
                break;
            }
            if let Some(until_date) = until {
                if current_date > until_date {
                    break;
                }
            }
            if let Some(max_count) = count {
                if iteration_count >= max_count {
                    break;
                }
            }

            if current_date >= range_start && current_date <= range_end {
                let new_start = current_date.and_hms_opt(start.time().hour(), start.time().minute(), start.time().second()).unwrap();
                occurrences.push((summary.to_string(), new_start, new_start + dur));
            }
        }
    }

    occurrences
}

fn parse_ics_content(ics_content: &str, range_start: NaiveDate, range_end: NaiveDate) -> Vec<(String, NaiveDateTime, NaiveDateTime)> {
    let mut events = Vec::new();
    let cursor = Cursor::new(ics_content.as_bytes());
    let parser = ical::parser::ical::IcalParser::new(cursor);

    for calendar in parser {
        if let Ok(cal) = calendar {
            for event in cal.events {
                let summary = event.get_property("SUMMARY")
                    .and_then(|p| p.value.as_deref())
                    .unwrap_or("No summary");

                let start_str = event.get_property("DTSTART")
                    .and_then(|p| p.value.as_deref())
                    .unwrap_or("");
                let start = parse_ical_datetime(start_str).unwrap_or_else(|_| {
                    DateTime::<Utc>::from_timestamp(0, 0).unwrap().naive_utc()
                });

                let end_str = event.get_property("DTEND")
                    .and_then(|p| p.value.as_deref())
                    .unwrap_or("");
                let end = parse_ical_datetime(end_str).unwrap_or_else(|_| {
                    DateTime::<Utc>::from_timestamp(0, 0).unwrap().naive_utc()
                });

                if let Some(rrule) = extract_rrule(&event) {
                    let expanded = expand_recurring_event(summary, start, end, &rrule, range_start, range_end);
                    events.extend(expanded);
                } else {
                    let event_date = start.date();
                    if event_date >= range_start && event_date <= range_end {
                        events.push((summary.to_string(), start, end));
                    }
                }
            }
        }
    }

    events
}

// --- CalDAV Request ---

pub async fn fetch_caldav_events(config: &CalDavConfig) -> Result<String, String> {
    let client = reqwest::Client::new();

    let xml_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

    let url = config.calendar_url();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/xml; charset=utf-8"));

    let auth = format!("{}:{}", config.login, config.password);
    let auth_header = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(auth));
    headers.insert("Authorization", HeaderValue::from_str(&auth_header).map_err(|e| e.to_string())?);
    headers.insert("Depth", HeaderValue::from_static("1"));

    let response = client
        .request(Method::from_bytes(b"REPORT").unwrap(), &url)
        .headers(headers)
        .body(xml_body)
        .send()
        .await
        .map_err(|e| format!("Erreur requete CalDAV: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Erreur CalDAV: status={}",
            response.status()
        ));
    }

    response.text().await.map_err(|e| format!("Erreur lecture reponse: {}", e))
}

// --- Extract ICS from XML ---

fn extract_ics_from_xml(xml_content: &str) -> Vec<String> {
    let mut ics_contents = Vec::new();
    let start_tags = ["<C:calendar-data>", "<cal:calendar-data>"];
    let end_tags = ["</C:calendar-data>", "</cal:calendar-data>"];
    let mut content = xml_content;

    for start_tag in &start_tags {
        while let Some(start_pos) = content.find(start_tag) {
            let mut found_end = false;
            for end_tag in &end_tags {
                if let Some(end_pos) = content[start_pos..].find(end_tag) {
                    let absolute_end = start_pos + end_pos;
                    let ics_data = &content[start_pos + start_tag.len()..absolute_end];
                    if !ics_data.trim().is_empty() {
                        ics_contents.push(ics_data.to_string());
                    }
                    content = &content[absolute_end + end_tag.len()..];
                    found_end = true;
                    break;
                }
            }
            if !found_end {
                break;
            }
        }
    }
    ics_contents
}

// --- Group by date ---

fn group_by_date(events: Vec<(String, NaiveDateTime, NaiveDateTime)>) -> CalendarResponse {
    let mut map: HashMap<String, Vec<Event>> = HashMap::new();

    for (summary, start, end) in events {
        let date_key = start.date().format("%Y-%m-%d").to_string();
        map.entry(date_key).or_default().push(Event {
            summary,
            start: format_display_date(&start),
            end: format_display_date(&end),
        });
    }

    CalendarResponse { events_by_date: map }
}

// --- Main ---

/// Récupère les événements de la semaine glissante sous forme brute,
/// avant formatage. Utilisé par l'endpoint events et par le module slots.
pub async fn get_sliding_week_raw_events(
    config: &CalDavConfig,
) -> Vec<(String, NaiveDateTime, NaiveDateTime)> {
    let (range_start, range_end) = get_sliding_week_range();

    match fetch_caldav_events(config).await {
        Ok(xml_response) => {
            let ics_contents = extract_ics_from_xml(&xml_response);
            let mut all_events = Vec::new();
            for ics in ics_contents {
                all_events.extend(parse_ics_content(&ics, range_start, range_end));
            }
            all_events
        }
        Err(_) => Vec::new(),
    }
}

pub async fn get_sliding_week_calendar_events(config: &CalDavConfig) -> CalendarResponse {
    group_by_date(get_sliding_week_raw_events(config).await)
}
