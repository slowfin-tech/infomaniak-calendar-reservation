use actix_web::{get, web, App, HttpServer, Responder};
use base64::Engine;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Europe::Zurich;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde::Serialize;
use std::env;

// --- Modèles de données ---

#[derive(Debug, Serialize)]
struct Event {
    summary: String,
    start: String,
    end: String,
}

#[derive(Debug, Serialize)]
struct WeekEvents {
    lundi: Vec<Event>,
    mardi: Vec<Event>,
    mercredi: Vec<Event>,
    jeudi: Vec<Event>,
    vendredi: Vec<Event>,
    samedi: Vec<Event>,
    dimanche: Vec<Event>,
}

// --- Configuration CalDAV ---

struct CalDavConfig {
    url: String,
    calendar_uri: String,
    login: String,
    password: String,
}

impl CalDavConfig {
    fn from_env() -> Self {
        CalDavConfig {
            url: env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".into()),
            calendar_uri: env::var("CALDAV_CALENDAR_URI").expect("CALDAV_CALENDAR_URI doit être défini"),
            login: env::var("CALDAV_LOGIN").expect("CALDAV_LOGIN doit être défini"),
            password: env::var("CALDAV_PASSWORD").expect("CALDAV_PASSWORD doit être défini"),
        }
    }

    fn calendar_url(&self) -> String {
        format!("{}{}", self.url.trim_end_matches('/'), self.calendar_uri)
    }
}

// --- Helper: Gestion des dates pour la semaine prochaine ---

fn get_next_week_range() -> (NaiveDate, NaiveDate) {
    let today = Local::now().date_naive();
    
    // Trouver le lundi de la semaine en cours
    let days_to_monday = today.weekday().num_days_from_monday() as i32;
    let days_to_monday_abs = days_to_monday.abs() as i64;
    let current_monday = today - Duration::days(days_to_monday_abs);
    
    // Lundi et dimanche de la semaine PROCHAINE
    let next_monday = current_monday + Duration::weeks(1);
    let next_sunday = next_monday + Duration::days(6);
    
    (next_monday, next_sunday)
}

fn format_display_date(dt: &NaiveDateTime) -> String {
    // Convertir en Zurich timezone pour affichage
    let zurich = Zurich.from_utc_datetime(dt);
    zurich.format("%Y-%m-%d %H:%M:%S").to_string()
}

// --- Parser ICS manuel ---

/// Extrait la valeur d'un champ ICS après le dernier ':'
/// Gère les formats:
/// - DTSTART:20260927T164500
/// - DTSTART;TZID=Europe/Zurich:20260927T164500
/// - DTSTART:20260927T164500Z
fn extract_ics_value(line: &str, prefix: &str) -> Option<String> {
    if !line.starts_with(prefix) {
        return None;
    }
    
    // Trouver le dernier ':' dans la ligne
    let last_colon = line.rfind(':')?;
    let value_part = &line[last_colon + 1..];
    
    if value_part.is_empty() {
        return None;
    }
    
    Some(value_part.trim().to_string())
}

fn parse_ics_content(ics_content: &str) -> Vec<(String, NaiveDateTime, NaiveDateTime)> {
    let mut events = Vec::new();
    let mut current_summary: Option<String> = None;
    let mut current_start: Option<NaiveDateTime> = None;
    let mut current_end: Option<NaiveDateTime> = None;
    let mut in_vevent = false;
    
    for line in ics_content.lines() {
        let trimmed = line.trim();
        
        if trimmed == "BEGIN:VEVENT" {
            in_vevent = true;
            current_summary = None;
            current_start = None;
            current_end = None;
        } else if trimmed == "END:VEVENT" {
            if let (Some(summary), Some(start), Some(end)) = (current_summary.clone(), current_start, current_end) {
                events.push((summary, start, end));
            }
            in_vevent = false;
        } else if in_vevent {
            if let Some(value) = extract_ics_value(trimmed, "SUMMARY:") {
                current_summary = Some(value);
            } else if let Some(value) = extract_ics_value(trimmed, "DTSTART") {
                if let Ok(dt) = parse_ics_datetime(&value) {
                    current_start = Some(dt);
                }
            } else if let Some(value) = extract_ics_value(trimmed, "DTEND") {
                if let Ok(dt) = parse_ics_datetime(&value) {
                    current_end = Some(dt);
                }
            }
        }
    }
    
    events
}

fn parse_ics_datetime(s: &str) -> Result<NaiveDateTime, ()> {
    let cleaned = s.trim_end_matches('Z');
    
    if cleaned.len() == 15 {
        // Format: YYYYMMDDTHHMMSS (avec ou sans Z à la fin)
        NaiveDateTime::parse_from_str(cleaned, "%Y%m%dT%H%M%S").map_err(|_| ())
    } else if cleaned.len() == 8 {
        // Format: YYYYMMDD (date seulement)
        NaiveDate::parse_from_str(cleaned, "%Y%m%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .map_err(|_| ())
    } else {
        Err(())
    }
}

// --- Requête CalDAV ---

async fn fetch_caldav_events(config: &CalDavConfig) -> Result<String, String> {
    let client = reqwest::Client::new();
    
    let (start_date, end_date) = get_next_week_range();
    
    let start_str = start_date.format("%Y%m%dT000000Z").to_string();
    let end_str = (end_date + Duration::days(1)).format("%Y%m%dT000000Z").to_string();
    
    let xml_body = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="{}" end="{}"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#,
        start_str, end_str
    );
    
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
        .map_err(|e| format!("Erreur requête CalDAV: {}", e))?;
    
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
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;
    
    Ok(body)
}

// --- Extraction des données ICS de la réponse XML CalDAV ---

fn extract_ics_from_xml(xml_content: &str) -> Vec<String> {
    let mut ics_contents = Vec::new();
    let mut in_calendar_data = false;
    let mut current_buffer = String::new();
    
    for line in xml_content.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("<C:calendar-data") {
            in_calendar_data = true;
            current_buffer.clear();
        } else if in_calendar_data && trimmed == "</C:calendar-data>" {
            if !current_buffer.is_empty() {
                ics_contents.push(current_buffer.clone());
            }
            in_calendar_data = false;
            current_buffer.clear();
        } else if in_calendar_data {
            current_buffer.push_str(line);
            current_buffer.push('\n');
        }
    }
    
    ics_contents
}

// --- Traitement et groupement des événements ---

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
        let zurich = Zurich.from_utc_datetime(&start);
        let local_date = zurich.date_naive();
        
        let days_since_monday = (local_date - start_date).num_days();
        
        if days_since_monday >= 0 && days_since_monday < 7 {
            match days_since_monday {
                0 => week.lundi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                1 => week.mardi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                2 => week.mercredi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                3 => week.jeudi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                4 => week.vendredi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                5 => week.samedi.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                6 => week.dimanche.push(Event { summary, start: format_display_date(&start), end: format_display_date(&end) }),
                _ => {}
            }
        }
    }
    
    week
}

// --- Handler de l'API ---

#[get("/api/calendar/events")]
async fn get_calendar_events() -> impl Responder {
    let config = CalDavConfig::from_env();
    
    match fetch_caldav_events(&config).await {
        Ok(xml_response) => {
            let ics_contents = extract_ics_from_xml(&xml_response);
            
            let mut all_events = Vec::new();
            for ics in ics_contents {
                let events = parse_ics_content(&ics);
                all_events.extend(events);
            }
            
            let week_events = group_events_by_day(all_events);
            web::Json(week_events)
        }
        Err(e) => {
            eprintln!("Erreur: {}", e);
            web::Json(WeekEvents {
                lundi: Vec::new(),
                mardi: Vec::new(),
                mercredi: Vec::new(),
                jeudi: Vec::new(),
                vendredi: Vec::new(),
                samedi: Vec::new(),
                dimanche: Vec::new(),
            })
        }
    }
}

// --- Point d'entrée ---

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    
    println!("Démarrage du serveur...");
    println!("Endpoint: GET /api/calendar/events");
    
    HttpServer::new(|| {
        App::new()
            .service(get_calendar_events)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
