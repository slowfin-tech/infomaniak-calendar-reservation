//! Booking module: reserve a SAV slot by creating an event on the Infomaniak
//! calendar via the "Plan a conference" API (POST /1/kmeet/rooms), which adds
//! the event on the calendar with the meeting URL.

use actix_web::{get, post, web, HttpRequest, HttpResponse};
use chrono::{Datelike, Duration, Local, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::calendar::{get_sliding_week_raw_events, get_sliding_week_range, CalDavConfig};
use crate::slots::{is_booked, respects_min_delay, SlotsConfig};

const INFOMANIAK_KMEET_URL: &str = "https://api.infomaniak.com/1/kmeet/rooms";
const TIMEZONE: &str = "Europe/Zurich";

/// Modele de description par defaut (comportement historique).
const DEFAULT_DESCRIPTION_TEMPLATE: &str = "{description}\n\nContact: {email}";

/// Applique les placeholders du modele de description:
/// {description}, {email}, {name}, {start}, {end}.
fn render_description(
    template: &str,
    request: &BookingRequest,
    start: &str,
    end: &str,
) -> String {
    template
        .replace("{description}", request.description.trim())
        .replace("{email}", request.email.trim())
        .replace("{name}", request.name.trim())
        .replace("{start}", start)
        .replace("{end}", end)
}

// --- Models ---

#[derive(Debug, Deserialize)]
pub struct BookingRequest {
    pub name: String,
    pub email: String,
    pub description: String,
    pub slot_id: String,
}

#[derive(Debug, Serialize)]
pub struct BookingResponse {
    pub slot_id: String,
    pub name: String,
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
    /// calendar_id/hostname depuis config.toml, token (secret) depuis .env.
    fn from_env() -> Self {
        let settings = &crate::config::global().infomaniak;
        InfomaniakConfig {
            api_token: std::env::var("KMEET_API_TOKEN").expect("KMEET_API_TOKEN doit etre defini"),
            calendar_id: settings.calendar_id,
            hostname: settings.visio_base_url.clone(),
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

/// Le slot_id designe-t-il un vrai debut de creneau pour le jour concerne
/// (grille configuree: duree et periodes via SAV_SLOT_DURATION / SAV_PERIODS) ?
fn is_valid_slot_start(config: &SlotsConfig, slot_start: &NaiveDateTime) -> bool {
    let weekday = slot_start.date().weekday().num_days_from_monday() as usize;
    config.is_valid_start(weekday, slot_start.hour(), slot_start.minute())
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// --- Infomaniak API ---

async fn create_calendar_event(
    config: &InfomaniakConfig,
    request: &BookingRequest,
    slot_start: NaiveDateTime,
    duration_minutes: i64,
) -> Result<serde_json::Value, String> {
    let slot_end = slot_start + Duration::minutes(duration_minutes);
    let title = truncate(&format!("SAV - {}", request.email.trim()), 150);
    let start = slot_start.format("%Y-%m-%d %H:%M:%S").to_string();
    let end = slot_end.format("%Y-%m-%d %H:%M:%S").to_string();
    let description_template = crate::config::global()
        .booking
        .description_template
        .as_deref()
        .unwrap_or(DEFAULT_DESCRIPTION_TEMPLATE);
    let description = truncate(&render_description(description_template, request, &start, &end), 2000);
    // L'API attend un hostname seul, sans scheme ni slash final.
    let hostname = config
        .hostname
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();

    let body = json!({
        "calendar_id": config.calendar_id,
        "starting_at": start,
        "ending_at": end,
        "timezone": TIMEZONE,
        "hostname": hostname,
        "title": title,
        "description": description,
        // Le client en participant: il recoit l'invitation.
        "attendees": [{
            "address": request.email.trim(),
            "organizer": false,
            "name": request.name.trim(),
            "state": "NEEDS-ACTION"
        }],
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

// --- Prochain rendez-vous d'un utilisateur ---

#[derive(Debug, Deserialize)]
pub struct NextBookingQuery {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct CancelRequest {
    pub email: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct NextBooking {
    pub slot_id: String,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Serialize)]
struct NextBookingResponse {
    next_booking: Option<NextBooking>,
}

/// Titre d'un evenement de reservation SAV pour cet email ?
/// Les reservations sont creees avec le titre "SAV - {email}".
fn is_sav_event_for(summary: &str, email: &str) -> bool {
    let summary = summary.trim().to_lowercase();
    let Some(rest) = summary.strip_prefix("sav - ") else {
        return false;
    };
    rest.trim() == email
}

/// Retourne le prochain rendez-vous SAV (evenement futur le plus proche)
/// de cet utilisateur dans la semaine glissante.
fn find_next_booking(
    events: &[crate::calendar::RawEvent],
    email: &str,
    now: NaiveDateTime,
) -> Option<NextBooking> {
    let email = email.trim().to_lowercase();
    events
        .iter()
        .filter(|event| is_sav_event_for(&event.summary, &email) && event.start > now)
        .min_by_key(|event| event.start)
        .map(|event| NextBooking {
            slot_id: event.start.format("%Y%m%d%H%M").to_string(),
            start: event.start.format("%Y-%m-%d %H:%M:%S").to_string(),
            end: event.end.format("%Y-%m-%d %H:%M:%S").to_string(),
        })
}

/// Retourne l'UID de l'evenement correspondant au prochain rendez-vous
/// de cet utilisateur (pour suppression via CalDAV).
fn find_next_booking_uid(
    events: &[crate::calendar::RawEvent],
    email: &str,
    now: NaiveDateTime,
) -> Option<String> {
    let email = email.trim().to_lowercase();
    events
        .iter()
        .filter(|event| is_sav_event_for(&event.summary, &email) && event.start > now)
        .min_by_key(|event| event.start)
        .and_then(|event| event.uid.clone())
}

#[get("/api/bookings/next")]
pub async fn get_next_booking(req: HttpRequest, query: web::Query<NextBookingQuery>) -> HttpResponse {
    if let Err(resp) = crate::check_api_key(&req) {
        return resp;
    }

    let email = query.email.trim();
    if email.is_empty() || !email.contains('@') {
        return HttpResponse::BadRequest().json(json!({"error": "email invalide"}));
    }

    let config = CalDavConfig::from_env();
    let events = get_sliding_week_raw_events(&config).await;
    let now = Local::now().naive_local();

    HttpResponse::Ok().json(NextBookingResponse {
        next_booking: find_next_booking(&events, email, now),
    })
}

#[post("/api/bookings/cancel")]
pub async fn cancel_booking(req: HttpRequest, body: web::Json<CancelRequest>) -> HttpResponse {
    if let Err(resp) = crate::check_api_key(&req) {
        return resp;
    }

    let email = body.email.trim();
    if email.is_empty() || !email.contains('@') {
        return HttpResponse::BadRequest().json(json!({"error": "email invalide"}));
    }

    let config = CalDavConfig::from_env();
    let now = Local::now().naive_local();
    let events = get_sliding_week_raw_events(&config).await;

    let Some(uid) = find_next_booking_uid(&events, email, now) else {
        return HttpResponse::NotFound().json(json!({"error": "aucun rendez-vous a annuler"}));
    };

    match crate::calendar::delete_caldav_event(&config, &uid).await {
        Ok(_) => {
            // Verification: l'evenement doit avoir disparu du calendar.
            let events = get_sliding_week_raw_events(&config).await;
            if find_next_booking_uid(&events, email, now).is_some() {
                HttpResponse::BadGateway()
                    .json(json!({"error": "suppression non confirmee par le calendar"}))
            } else {
                HttpResponse::Ok().json(json!({"cancelled": true}))
            }
        }
        Err(e) => HttpResponse::BadGateway().json(json!({"error": e})),
    }
}

#[post("/api/bookings")]
pub async fn create_booking(req: HttpRequest, body: web::Json<BookingRequest>) -> HttpResponse {
    if let Err(resp) = crate::check_api_key(&req) {
        return resp;
    }

    let request = body.into_inner();
    let caldav_config = CalDavConfig::from_env();
    let infomaniak_config = InfomaniakConfig::from_env();
    let slots_config = SlotsConfig::from_app_config(crate::config::global());

    let Some(slot_start) = parse_slot_id(&request.slot_id) else {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id invalide, format attendu YYYYMMDDHHMM"}));
    };

    if request.name.trim().is_empty() {
        return HttpResponse::BadRequest().json(json!({"error": "name requis"}));
    }
    if request.email.trim().is_empty() || !request.email.contains('@') {
        return HttpResponse::BadRequest().json(json!({"error": "email invalide"}));
    }

    if !is_valid_slot_start(&slots_config, &slot_start) {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id ne correspond pas a un debut de creneau valide"}));
    }

    let (range_start, range_end) = get_sliding_week_range();
    let slot_end = slot_start + Duration::minutes(slots_config.duration_minutes);

    if slot_start.date() < range_start || slot_start.date() > range_end {
        return HttpResponse::BadRequest()
            .json(json!({"error": "slot_id hors de la semaine glissante"}));
    }
    let now = Local::now().naive_local();
    if slot_end <= now {
        return HttpResponse::BadRequest().json(json!({"error": "slot_id deja passe"}));
    }
    if !respects_min_delay(slot_start, now, slots_config.min_delay_minutes) {
        return HttpResponse::BadRequest().json(json!({
            "error": format!(
                "slot_id trop proche: reservation possible au moins {} minutes a l avance",
                slots_config.min_delay_minutes
            )
        }));
    }

    // Deux requetes concurrentes sur le meme slot : la premiere gagne,
    // la seconde recoit 409 sans toucher au calendar.
    if !try_claim(&request.slot_id) {
        return HttpResponse::Conflict().json(json!({"error": "reservation deja en cours pour ce slot"}));
    }

    let outcome = async {
        let events = get_sliding_week_raw_events(&caldav_config).await;

        // Un utilisateur ne peut avoir qu'un rendez-vous a la fois.
        if find_next_booking(&events, &request.email, now).is_some() {
            return HttpResponse::Conflict()
                .json(json!({"error": "un rendez-vous est deja reserve pour cet email"}));
        }

        if is_booked(&events, slot_start, slot_end) {
            return HttpResponse::Conflict().json(json!({"error": "slot deja reserve"}));
        }

        match create_calendar_event(
            &infomaniak_config,
            &request,
            slot_start,
            slots_config.duration_minutes,
        )
        .await
        {
            Ok(event) => HttpResponse::Created().json(BookingResponse {
                slot_id: request.slot_id.clone(),
                name: request.name.trim().to_string(),
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
        // Grille par defaut (30 min, 10h-12h / 14h-16h tous les jours).
        let config = SlotsConfig {
            duration_minutes: 30,
            min_delay_minutes: 0,
            periods: std::array::from_fn(|_| vec![((10, 0), (12, 0)), ((14, 0), (16, 0))]),
        };
        // Mardi 2026-09-29 (les periodes s'appliquent au jour de la date).
        let date = NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        assert!(is_valid_slot_start(&config, &date.and_hms_opt(14, 0, 0).unwrap()));
        assert!(is_valid_slot_start(&config, &date.and_hms_opt(10, 30, 0).unwrap()));
        assert!(!is_valid_slot_start(&config, &date.and_hms_opt(13, 0, 0).unwrap()));
        assert!(!is_valid_slot_start(&config, &date.and_hms_opt(14, 15, 0).unwrap()));
        assert!(!is_valid_slot_start(&config, &date.and_hms_opt(12, 0, 0).unwrap()));

        // Duree configuree a 60 min sur 14h-16h: debuts a 14h00 et 15h00.
        let hourly = SlotsConfig {
            duration_minutes: 60,
            min_delay_minutes: 0,
            periods: std::array::from_fn(|_| vec![((14, 0), (16, 0))]),
        };
        assert!(!is_valid_slot_start(&hourly, &date.and_hms_opt(14, 15, 0).unwrap()));
        assert!(!is_valid_slot_start(&hourly, &date.and_hms_opt(13, 0, 0).unwrap()));
        assert!(is_valid_slot_start(&hourly, &date.and_hms_opt(14, 0, 0).unwrap()));
        assert!(is_valid_slot_start(&hourly, &date.and_hms_opt(15, 0, 0).unwrap()));
    }

    #[test]
    fn next_booking_trouve_le_plus_proche() {
        use crate::calendar::RawEvent;
        let date = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let at = |h: u32| date.and_hms_opt(h, 0, 0).unwrap();
        let now = at(9);

        let event = |summary: &str, h1: u32, h2: u32, uid: Option<&str>| RawEvent {
            summary: summary.to_string(),
            start: at(h1),
            end: at(h2),
            uid: uid.map(|u| u.to_string()),
        };

        let events = vec![
            event("SAV - autre@domain.com", 10, 11, Some("uid-autre")), // autre utilisateur
            event("SAV - client@domain.com", 15, 16, Some("uid-tardif")), // plus tard
            event("SAV - client@domain.com", 11, 12, Some("uid-proche")), // doit gagner
            event("SAV - client@domain.com", 8, 9, Some("uid-passe")), // passe: ignore
            event("RDV equipe", 10, 11, None), // hors SAV
        ];

        let next = find_next_booking(&events, "Client@Domain.com ", now).unwrap();
        assert_eq!(next.slot_id, "202610051100");
        assert_eq!(next.start, "2026-10-05 11:00:00");
        assert_eq!(next.end, "2026-10-05 12:00:00");

        // UID du meme evenement (pour l'annulation).
        assert_eq!(find_next_booking_uid(&events, "client@domain.com", now).as_deref(), Some("uid-proche"));

        // Aucun rendez-vous futur pour cet email.
        let none = find_next_booking(&events, "inconnu@domain.com", now);
        assert!(none.is_none());
        assert!(find_next_booking_uid(&events, "inconnu@domain.com", now).is_none());
    }

    #[test]
    fn titre_sav_reconnu_insensible_casse() {
        assert!(is_sav_event_for("SAV - client@domain.com", "client@domain.com"));
        assert!(is_sav_event_for("sav - CLIENT@domain.com", "client@domain.com"));
        assert!(!is_sav_event_for("SAV - autre@domain.com", "client@domain.com"));
        assert!(!is_sav_event_for("SAV equipe", "client@domain.com"));
        assert!(!is_sav_event_for("client@domain.com", "client@domain.com"));
    }

    #[test]
    fn description_modele_defaut() {
        let request = BookingRequest {
            name: "Jean Dupont".to_string(),
            email: "client@domain.com".to_string(),
            description: "Probleme de connexion".to_string(),
            slot_id: "202610031400".to_string(),
        };
        let rendered = render_description(
            DEFAULT_DESCRIPTION_TEMPLATE,
            &request,
            "2026-10-03 14:00:00",
            "2026-10-03 14:30:00",
        );
        assert_eq!(rendered, "Probleme de connexion\n\nContact: client@domain.com");
    }

    #[test]
    fn description_modele_configure() {
        let request = BookingRequest {
            name: "Jean Dupont".to_string(),
            email: "client@domain.com".to_string(),
            description: "Probleme de connexion".to_string(),
            slot_id: "202610031400".to_string(),
        };
        let template = "Rendez-vous SAV\nProblème : {description}\nClient : {name} ({email})\nCréneau : {start} -> {end}";
        let rendered = render_description(template, &request, "2026-10-03 14:00:00", "2026-10-03 14:30:00");
        assert_eq!(
            rendered,
            "Rendez-vous SAV\nProblème : Probleme de connexion\nClient : Jean Dupont (client@domain.com)\nCréneau : 2026-10-03 14:00:00 -> 2026-10-03 14:30:00"
        );
    }

    #[test]
    fn description_modele_placeholder_inconnu_sans_effet() {
        let request = BookingRequest {
            name: "Jean".to_string(),
            email: "a@b.com".to_string(),
            description: "desc".to_string(),
            slot_id: "202610031400".to_string(),
        };
        let rendered = render_description("{description} {inconnu}", &request, "s", "e");
        assert_eq!(rendered, "desc {inconnu}");
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
