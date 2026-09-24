//! Module for fetching and processing CalDAV events from Infomaniak

use base64::Engine;
use chrono::{Datelike, DateTime, Duration, Local, NaiveDate, NaiveDateTime, Timelike, Utc};
use ical::parser::ical::component::IcalEvent;
use ical::parser::Component;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde::Serialize;
use std::io::Cursor;

// --- Public Models ---

/// Represents a calendar event with summary, start and end times
#[derive(Debug, Serialize, Clone)]
pub struct Event {
    pub summary: String,
    pub start: String,
    pub end: String,
}

/// Weekly events grouped by day (Monday to Sunday)
#[derive(Debug, Serialize, Clone)]
pub struct WeekEvents {
    pub lundi: Vec<Event>,
    pub mardi: Vec<Event>,
    pub mercredi: Vec<Event>,
    pub jeudi: Vec<Event>,
    pub vendredi: Vec<Event>,
    pub samedi: Vec<Event>,
    pub dimanche: Vec<Event>,
}

// --- CalDAV Configuration ---

/// Configuration for connecting to a CalDAV server
pub struct CalDavConfig {
    pub url: String,
    pub calendar_uri: String,
    pub login: String,
    pub password: String,
}

impl CalDavConfig {
    /// Creates a new CalDavConfig from environment variables
    ///
    /// Required variables:
    /// - CALDAV_CALENDAR_URI: The calendar URI path
    /// - CALDAV_LOGIN: The login/username
    /// - CALDAV_PASSWORD: The password
    ///
    /// Optional:
    /// - CALDAV_URL: The base URL (defaults to https://sync.infomaniak.com/)
    pub fn from_env() -> Self {
        CalDavConfig {
            url: std::env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".into()),
            calendar_uri: std::env::var("CALDAV_CALENDAR_URI").expect("CALDAV_CALENDAR_URI doit etre defini"),
            login: std::env::var("CALDAV_LOGIN").expect("CALDAV_LOGIN doit etre defini"),
            password: std::env::var("CALDAV_PASSWORD").expect("CALDAV_PASSWORD doit etre defini"),
        }
    }

    /// Returns the full calendar URL
    pub fn calendar_url(&self) -> String {
        format!("{}{}", self.url.trim_end_matches('/'), self.calendar_uri)
    }
}

// --- Date Helpers ---

/// Returns the start (Monday) and end (Sunday) dates of the next week
pub fn get_next_week_range() -> (NaiveDate, NaiveDate) {
    let today = Local::now().date_naive();
    let days_to_monday = today.weekday().num_days_from_monday() as i32;
    let days_to_monday_abs = days_to_monday.abs() as i64;
    let current_monday = today - Duration::days(days_to_monday_abs);
    let next_monday = current_monday + Duration::weeks(1);
    let next_sunday = next_monday + Duration::days(6);
    (next_monday, next_sunday)
}

/// Formats a NaiveDateTime for display
///
/// Note: Assumes the datetime is already in the correct timezone (Europe/Zurich)
/// as Infomaniak returns dates with TZID=Europe/Zurich
pub fn format_display_date(dt: &NaiveDateTime) -> String {
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

/// Expands a recurring event into all occurrences within the given date range
///
/// Supports:
/// - FREQ: DAILY, WEEKLY, MONTHLY, YEARLY
/// - INTERVAL parameter
/// - UNTIL constraint
/// - COUNT constraint
fn expand_recurring_event_simple(
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
        let max_iterations = 500;

        loop {
            if iteration_count >= max_iterations {
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
                    let expanded = expand_recurring_event_simple(summary, start, end, &rrule, range_start, range_end);
                    events.extend(expanded);
                } else {
                    let event_date = start.date();
                    if event_date >= range_start && event_date <= range_end {
                        events.extend(vec![(summary.to_string(), start, end)]);
                    }
                }
            }
        }
    }

    events
}

// --- CalDAV Request ---

/// Fetches raw CalDAV response from the server
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
            "Erreur CalDAV: status={}, body={:?}",
            response.status(),
            response.text().await.unwrap_or_else(|_| "N/A".into())
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Erreur lecture reponse: {}", e))?;

    Ok(body)
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

/// Groups events by day of the week
///
/// Assumes dates are already in local timezone (Europe/Zurich)
fn group_events_by_day(events: Vec<(String, NaiveDateTime, NaiveDateTime)>) -> WeekEvents {
    let mut week = WeekEvents {
        lundi: Vec::new(),
        mardi: Vec::new(),
        mercredi: Vec::new(),
        jeudi: Vec::new(),
        vendredi: Vec::new(),
        samedi: Vec::new(),
        dimanche: Vec::new(),
    };

    let (start_date, _) = get_next_week_range();

    for (summary, start, end) in events {
        let local_date = start.date();

        let days_since_monday = (local_date - start_date).num_days();

        if days_since_monday >= 0 && days_since_monday < 7 {
            let event = Event {
                summary,
                start: format_display_date(&start),
                end: format_display_date(&end),
            };
            match days_since_monday {
                0 => week.lundi.push(event),
                1 => week.mardi.push(event),
                2 => week.mercredi.push(event),
                3 => week.jeudi.push(event),
                4 => week.vendredi.push(event),
                5 => week.samedi.push(event),
                6 => week.dimanche.push(event),
                _ => {}
            }
        }
    }

    week
}

/// Main function to fetch and process calendar events for the next week
///
/// Returns a WeekEvents struct with all events grouped by day
pub async fn get_next_week_calendar_events(config: &CalDavConfig) -> WeekEvents {
    let (range_start, range_end) = get_next_week_range();

    match fetch_caldav_events(config).await {
        Ok(xml_response) => {
            let ics_contents = extract_ics_from_xml(&xml_response);

            let mut all_events = Vec::new();
            for ics in ics_contents {
                let events = parse_ics_content(&ics, range_start, range_end);
                all_events.extend(events);
            }

            group_events_by_day(all_events)
        }
        Err(_) => {
            WeekEvents {
                lundi: Vec::new(),
                mardi: Vec::new(),
                mercredi: Vec::new(),
                jeudi: Vec::new(),
                vendredi: Vec::new(),
                samedi: Vec::new(),
                dimanche: Vec::new(),
            }
        }
    }
}
