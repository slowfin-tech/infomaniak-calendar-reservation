//! Booking module: reserve a SAV slot by creating an event on the Infomaniak
//! calendar via the "Plan a conference" API (POST /1/kmeet/rooms), which adds
//! the event on the calendar with the meeting URL.

use actix_web::{post, web, HttpRequest, HttpResponse};
use chrono::{Duration, Local, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::calendar::{get_sliding_week_raw_events, get_sliding_week_range, CalDavConfig};
use crate::slots::{is_booked, slot_starts, SLOT_DURATION_MINUTES};

const INFOMANIAK_KMEET_URL: &str = "https://api.infomaniak.com/1/kmeet/rooms";
const TIMEZONE: &str = "Europe/Zurich";

// --- Models ---

#[derive(Debug, Deserialize)]
pub struct BookingRequest {
    pub email: String,
    pub description: String,
    pub slot_id: String,
}

#[derive(Debug, Serialize)]
pub struct BookingResponse {
    pub slot_id: String,
    pub email: String,
    pub description: String,
    pub start: String,
    pub end: String,
    /// Donnees brutes retournees par l'API Infomaniak (room, url, event_id...).
    pub event: serde_json::Value,
}

#[derive(Debug)]
struct InfomaniakConfig {
    api_token: String,
    calendar_id: u64,
    hostname: String,
}

impl InfomaniakConfig {
    fn from_env() -> Self {
        InfomaniakConfig {
            api_token: std::env::var("KMEET_API_TOKEN").expect("KMEET_API_TOKEN doit etre defini"),
            calendar_id: std::env::var("KCALENDAR_ID")
                .expect("KCALENDAR_ID doit etre defini")
                .parse()
                .expect("KCALENDAR_ID doit etre un entier"),
            hostname: std::env::var("VISIO_BASE_URL").expect("VISIO_BASE_URL doit etre defini"),
        }
    }
}

// --- Slot ID helpers ---

/// Parse un slot_id au format YYYYMMDDHHMM.
pub fn parse_slot_id(slot_id: &str) -> Option<NaiveDateTime> {
    if slot_id.len() != 12 || !slot_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    NaiveDateTime::parse_from_str(slot_id, "%Y%m%d%H%M").ok()
}

/// Le slot_id designe-t-il un vrai debut de creneau (grille 10h-12h / 14h-16h) ?
fn is_valid_slot_start(slot_start: &NaiveDateTime) -> bool {
    slot_starts().contains(&(slot_start.hour(), slot_start.minute()))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// --- Infomaniak API ---

async fn create_calendar_event(
    config: &InfomaniakConfig,
    request: &BookingRequest,
    slot_start: NaiveDateTime,
) -> Result<serde_json::Value, String> {
    let slot_end = slot_start + Duration::minutes(SLOT_DURATION_MINUTES);
    let title = truncate(&format!("SAV - {}", request.email.trim()), 150);
    let description = truncate(
        &format!("{}\n\nContact: {}", request.description.trim(), request.email.trim()),
        2000,
    );
    // L'API attend un hostname seul, sans scheme ni slash final.
    let hostname = config
        .hostname
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();

    let body = json!({
        "calendar_id": config.calendar_id,
        "starting_at": slot_start.format("%Y-%m-%d %H:%M:%S").to_string(),
        "ending_at": slot_end.format("%Y-%m-%d %H:%M:%S").to_string(),
        "timezone": TIMEZONE,
        "hostname": hostname,
        "title": title,
        "description": description,
        "options": {
            "subject": title,
            "start_audio_muted": false,
            "enable_moderator_video": false,
            "start_audio_only": false,
            "lobby_enabled": false,
            "password_enabled": false,
            "e2ee_enabled": false
        }
    });

    let client = reqwest::Client::new();
    let response = client
        .post(INFOMANIAK_KMEET_URL)
        .bearer_auth(&config.api_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Erreur requete Infomaniak: {}", e))?;

    let status = response.status();
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture reponse Infomaniak: {}", e))?;

    if !status.is_success() {
        return Err(format!("Erreur API Infomaniak: status={} body={}", status, json));
    }
    match json.get("result").and_then(|r| r.as_str()) {
        Some("success") | Some("asynchronous") => Ok(json.get("data").cloned().unwrap_or(serde_json::Value::Null)),
        _ => Err(format!("Erreur API Infomaniak: body={}", json)),
    }
}

// --- Handler ---

/// Slots actuellement en cours de reservation, proteges contre les courses
/// entre la verification CalDAV et la creation de l'evenement.
fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static IN_FLIGHT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Reserve atomiquement le slot. Retourne false si une reservation est deja
/// en cours pour ce slot.
fn try_claim(slot_id: &str) -> bool {
    in_flight().lock().unwrap().insert(slot_id.to_string())
}

fn release(slot_id: &str) {
    in_flight().lock().unwrap().remove(slot_id);
}

#[post("/api/bookings")]
pub async fn create_booking(req: HttpRequest, body: web::Json<BookingRequest>) -> HttpResponse {
    if let Err(resp) = crate::check_api_key(&req) {
        return resp;
    }

    let request = body.into_inner();
    let caldav_config = CalDavConfig::from_env();
    let infomaniak_config = InfomaniakConfig::from_env();

    let Some(slot_start) = parse_slot_id(&request.slot_id) else {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id invalide, format attendu YYYYMMDDHHMM"}));
    };

    if request.email.trim().is_empty() || !request.email.contains('@') {
        return HttpResponse::BadRequest().json(json!({"error": "email invalide"}));
    }

    if !is_valid_slot_start(&slot_start) {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id ne correspond pas a un debut de creneau (10h-12h ou 14h-16h, par tranche de 30 min)"}));
    }

    let (range_start, range_end) = get_sliding_week_range();
    let slot_end = slot_start + Duration::minutes(SLOT_DURATION_MINUTES);

    if slot_start.date() < range_start || slot_start.date() > range_end {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id hors de la semaine glissante"}));
    }
    if slot_end <= Local::now().naive_local() {
        return HttpResponse::BadRequest().json(json!({"error": "slot_id deja passe"}));
    }

    // Deux requetes concurrentes sur le meme slot : la premiere gagne,
    // la seconde recoit 409 sans toucher au calendar.
    if !try_claim(&request.slot_id) {
        return HttpResponse::Conflict().json(json!({"error": "reservation deja en cours pour ce slot"}));
    }

    let outcome = async {
        let events = get_sliding_week_raw_events(&caldav_config).await;
        if is_booked(&events, slot_start, slot_end) {
            return HttpResponse::Conflict().json(json!({"error": "slot deja reserve"}));
        }

        match create_calendar_event(&infomaniak_config, &request, slot_start).await {
            Ok(event) => HttpResponse::Created().json(BookingResponse {
                slot_id: request.slot_id.clone(),
                email: request.email.trim().to_string(),
                description: request.description.trim().to_string(),
                start: slot_start.format("%Y-%m-%d %H:%M:%S").to_string(),
                end: slot_end.format("%Y-%m-%d %H:%M:%S").to_string(),
                event,
            }),
            Err(e) => HttpResponse::BadGateway().json(json!({"error": e})),
        }
    }
    .await;

    release(&request.slot_id);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parse_slot_id_valid() {
        assert_eq!(
            parse_slot_id("202609291430").unwrap(),
            NaiveDate::from_ymd_opt(2026, 9, 29).unwrap().and_hms_opt(14, 30, 0).unwrap()
        );
    }

    #[test]
    fn parse_slot_id_invalid() {
        assert!(parse_slot_id("2026092914").is_none());
        assert!(parse_slot_id("202609291430A").is_none());
        assert!(parse_slot_id("2026-09-29 14:30").is_none());
    }

    #[test]
    fn slot_must_start_on_grid() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        assert!(is_valid_slot_start(&date.and_hms_opt(14, 0, 0).unwrap()));
        assert!(is_valid_slot_start(&date.and_hms_opt(10, 30, 0).unwrap()));
        assert!(!is_valid_slot_start(&date.and_hms_opt(13, 0, 0).unwrap()));
        assert!(!is_valid_slot_start(&date.and_hms_opt(14, 15, 0).unwrap()));
        assert!(!is_valid_slot_start(&date.and_hms_opt(12, 0, 0).unwrap()));
    }

    #[test]
    fn claim_is_exclusive_then_released() {
        assert!(try_claim("202609291430"));
        // Une seconde reservation concurrente du meme slot est refusee.
        assert!(!try_claim("202609291430"));
        // Un autre slot n'est pas bloque.
        assert!(try_claim("202609291500"));

        release("202609291430");
        // Apres liberation (fin de la premiere requete), le slot redevient
        // reservable - le calendar aura pris le relais si elle a abouti.
        assert!(try_claim("202609291430"));

        release("202609291430");
        release("202609291500");
    }
}
