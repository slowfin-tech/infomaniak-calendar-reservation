use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder, Result};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc, Datelike, Duration};
use dotenvy::dotenv;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

// ============================================================================
// Query Parameters
// ============================================================================

/// Paramètres de requête pour /api/slots
#[derive(Debug, Deserialize)]
struct SlotsQueryParams {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

// ============================================================================
// Models
// ============================================================================

/// Représente un créneau disponible pour un rendez-vous
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Slot {
    /// Identifiant unique du slot
    id: String,
    /// Date du rendez-vous (format YYYY-MM-DD)
    date: String,
    /// Heure de début (format HH:MM)
    start_time: String,
    /// Heure de fin (format HH:MM)
    end_time: String,
    /// Lien visio pour ce slot
    visio_link: String,
    /// Statut de réservation
    booked: bool,
}

impl Slot {
    /// Crée un nouveau slot
    fn new(date: NaiveDate, start_time: NaiveTime, visio_link: String) -> Self {
        let end_time = start_time + Duration::minutes(30);
        let id = format!("{}-{}", date.format("%Y%m%d"), start_time.format("%H%M"));
        
        Slot {
            id,
            date: date.format("%Y-%m-%d").to_string(),
            start_time: start_time.format("%H:%M").to_string(),
            end_time: end_time.format("%H:%M").to_string(),
            visio_link,
            booked: false,
        }
    }
}

// ============================================================================
// App State
// ============================================================================

/// État partagé de l'application
struct AppState {
    /// Map des slots réservés (slot_id -> (kmeet_url, booked))
    booked_slots: Mutex<HashMap<String, (Option<String>, bool)>>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            booked_slots: Mutex::new(HashMap::new()),
        }
    }
    
    /// Vérifie si un slot est réservé
    fn is_booked(&self, slot_id: &str) -> bool {
        let booked = self.booked_slots.lock().unwrap();
        booked.get(slot_id).map(|(_, booked)| *booked).unwrap_or(false)
    }
    
    /// Réserve un slot avec optionnellement une URL KMeet
    fn book_slot(&self, slot_id: String, kmeet_url: Option<String>) -> bool {
        let mut booked = self.booked_slots.lock().unwrap();
        booked.insert(slot_id, (kmeet_url, true));
        true
    }
    
    /// Récupère l'URL KMeet pour un slot réservé
    fn get_kmeet_url(&self, slot_id: &str) -> Option<String> {
        let booked = self.booked_slots.lock().unwrap();
        booked.get(slot_id).and_then(|(url, _)| url.clone())
    }
    
    /// Vérifie si un slot peut être réservé (minimum 2 jours à l'avance)
    fn can_book(&self, slot_id: &str, slot_date: &str) -> bool {
        // Vérifier si déjà réservé
        if self.is_booked(slot_id) {
            return false;
        }
        
        // Vérifier la date minimum (2 jours à l'avance)
        let slot_naive = match NaiveDate::parse_from_str(slot_date, "%Y-%m-%d") {
            Ok(date) => date,
            Err(_) => return false,
        };
        
        let today = Utc::now().date_naive();
        let min_date = today + Duration::days(2);
        
        // Le slot doit être au moins 2 jours après aujourd'hui
        slot_naive >= min_date
    }
}

// ============================================================================
// Calendar Integration
// ============================================================================

/// Structure pour représenter un événement du calendrier
#[derive(Debug, Clone)]
struct CalendarEvent {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    summary: Option<String>,
    /// Règle de récurrence (RRULE) si l'événement est récurrent
    rrule: Option<String>,
    /// Date de début de la récurrence
    dtstart_raw: Option<String>,
    /// Durée de l'événement (si DURATION est utilisé au lieu de DTEND)
    duration: Option<chrono::Duration>,
}

/// Parse une ligne DTSTART ou DTEND au format ICS
/// Gère les formats:
/// - 20260912T140000Z (UTC avec Z)
/// - 20260912T140000 (local, on assume UTC)
/// - DTSTART;TZID=Europe/Paris:20260912T140000 (avec timezone)
fn parse_ics_datetime(dt_str: &str) -> Option<DateTime<Utc>> {
    // Extraire la valeur après le dernier ':'
    let value = dt_str.split(':').last()?;
    
    let has_z = value.ends_with('Z');
    let clean_str = if has_z { value.trim_end_matches('Z') } else { value };
    
    if clean_str.len() == 15 {
        // Format: YYYYMMDDTHHMMSS (15 chars)
        let date_part = &clean_str[..8];  // YYYYMMDD
        let time_part = &clean_str[9..15]; // HHMMSS (skip T at position 8)
        let date = NaiveDate::parse_from_str(date_part, "%Y%m%d").ok()?;
        let time = NaiveTime::parse_from_str(time_part, "%H%M%S").ok()?;
        Some(Utc.from_utc_datetime(&date.and_time(time)))
    } else {
        None
    }
}

/// Parse une durée au format ISO 8601 (ex: PT30M, PT1H30M, P1D)
fn parse_ics_duration(duration_str: &str) -> Option<chrono::Duration> {
    // Supprimer les paramètres comme FREQ=WEEKLY;INTERVAL=1
    let clean_str = duration_str.split(';').next()?.trim();
    
    if !clean_str.starts_with("PT") && !clean_str.starts_with("P") {
        return None;
    }
    
    let duration_str = clean_str.trim_start_matches("PT").trim_start_matches("P");
    let mut total_seconds = 0i64;
    let mut remaining = duration_str;
    
    // Parse hours
    if let Some(pos) = remaining.find('H') {
        let hours: i64 = remaining[..pos].parse().ok()?;
        total_seconds += hours * 3600;
        remaining = &remaining[pos+1..];
    }
    
    // Parse minutes
    if let Some(pos) = remaining.find('M') {
        let minutes: i64 = remaining[..pos].parse().ok()?;
        total_seconds += minutes * 60;
        remaining = &remaining[pos+1..];
    }
    
    // Parse seconds
    if let Some(pos) = remaining.find('S') {
        let seconds: i64 = remaining[..pos].parse().ok()?;
        total_seconds += seconds;
    }
    
    Some(chrono::Duration::seconds(total_seconds))
}

/// Parse un fichier ICS et extrait les événements
/// Gère les événements simples et récurrents (RRULE)
fn parse_ics_content(content: &str) -> Vec<CalendarEvent> {
    let mut events = Vec::new();
    let mut current_event: Option<CalendarEvent> = None;
    
    for line in content.lines() {
        let line = line.trim();
        // Skip empty lines and continuation lines (lines starting with space)
        if line.is_empty() {
            continue;
        }
        // Handle folded lines (lines that are continuations)
        if line.starts_with(' ') {
            // For now, we skip continuation lines as we handle the main format
            // A more robust parser would unfold these
            continue;
        }
        
        if line == "BEGIN:VEVENT" {
            current_event = Some(CalendarEvent {
                start: Utc::now(),
                end: Utc::now(),
                summary: None,
                rrule: None,
                dtstart_raw: None,
                duration: None,
            });
        } else if line == "END:VEVENT" {
            if let Some(event) = current_event.take() {
                events.push(event);
            }
        } else if let Some(ref mut event) = current_event {
            if line.starts_with("DTSTART:") || line.starts_with("DTSTART;") {
                let dt_str = &line[7..]; // Remove "DTSTART" prefix
                event.start = parse_ics_datetime(dt_str).unwrap_or(Utc::now());
                event.dtstart_raw = Some(dt_str.to_string());
            } else if line.starts_with("DTEND:") || line.starts_with("DTEND;") {
                let dt_str = &line[5..]; // Remove "DTEND" prefix
                event.end = parse_ics_datetime(dt_str).unwrap_or(Utc::now());
            } else if line.starts_with("SUMMARY:") || line.starts_with("SUMMARY;") {
                let summary = &line[7..]; // Remove "SUMMARY" prefix
                event.summary = Some(summary.to_string());
            } else if line.starts_with("RRULE:") || line.starts_with("RRULE;") {
                let rrule = &line[6..]; // Remove "RRULE" prefix
                event.rrule = Some(rrule.to_string());
            } else if line.starts_with("DURATION:") || line.starts_with("DURATION;") {
                let duration_str = &line[9..]; // Remove "DURATION" prefix
                // Parse ISO 8601 duration format like PT30M, PT1H, etc.
                if let Some(dur) = parse_ics_duration(duration_str) {
                    event.duration = Some(dur);
                }
            }
        }
    }
    
    events
}

/// Récupère le calendrier ICS depuis l'URL configurée
async fn fetch_calendar_events() -> Vec<CalendarEvent> {
    let calendar_url = match get_kcalendar_url() {
        Some(url) => url,
        None => return Vec::new(),
    };
    
    let client = reqwest::Client::new();
    match client.get(&calendar_url).send().await {
        Ok(response) => {
            if response.status().is_success() {
                let content = match response.text().await {
                    Ok(text) => text,
                    Err(_) => return Vec::new(),
                };
                let events = parse_ics_content(&content);
                // Expandir les événements récurrents
                expand_recurring_events(events)
            } else {
                eprintln!("Failed to fetch calendar: HTTP {}", response.status());
                Vec::new()
            }
        }
        Err(e) => {
            eprintln!("Failed to fetch calendar: {}", e);
            Vec::new()
        },
    }
}

/// Étend les événements récurrents en générant toutes leurs occurrences
/// dans une plage raisonnable (par défaut: 2 ans autour de la date actuelle)
fn expand_recurring_events(events: Vec<CalendarEvent>) -> Vec<CalendarEvent> {
    let mut expanded_events = Vec::new();
    let now = Utc::now().date_naive();
    
    for event in events {
        // Si l'événement a une règle de récurrence (RRULE)
        if let Some(rrule) = &event.rrule {
            // Pour l'instant, gérons seulement les RRULE simples de type FREQ=WEEKLY
            // Un parseur complet RRULE serait plus complexe
            
            if rrule.contains("FREQ=WEEKLY") {
                // Extraire l'intervalle (par défaut 1)
                let interval = if rrule.contains("INTERVAL=") {
                    rrule.split("INTERVAL=").nth(1)
                        .and_then(|s| s.split(';').next())
                        .and_then(|s| s.parse::<i32>().ok())
                        .unwrap_or(1)
                } else {
                    1
                };
                
                // Date de début de la récurrence
                let start_date = event.start.date_naive();
                
                // Date de fin si UNTIL est spécifié
                let end_date = if rrule.contains("UNTIL=") {
                    // Extraire la date UNTIL
                    if let Some(until_str) = rrule.split("UNTIL=").nth(1).and_then(|s| s.split(';').next()) {
                        // Parse UNTIL date (format: 20260812T215959Z)
                        if let Some(until_dt) = parse_ics_datetime(until_str) {
                            Some(until_dt.date_naive())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                
                // Générer des occurrences à partir de start_date jusqu'à 1 an dans le futur
                // ou jusqu'à end_date si spécifié
                let mut current_date = start_date;
                let mut max_iterations = 1000; // Sécurité pour éviter les boucles infinies
                
                while max_iterations > 0 {
                    max_iterations -= 1;
                    
                    // Si nous avons une date de fin et que current_date la dépasse, arrêter
                    if let Some(end) = end_date {
                        if current_date > end {
                            break;
                        }
                    } else {
                        // Sans date de fin, générer jusqu'à 2 ans dans le futur
                        if current_date > now + Duration::days(365 * 2) {
                            break;
                        }
                    }
                    
                    // Conserver le même temps de la journée que l'événement original
                    let original_time = event.start.time();
                    let new_start = Utc.from_utc_datetime(&current_date.and_time(original_time));
                    
                    // Calculer la fin en utilisant la durée de l'événement original
                    let duration = event.end - event.start;
                    let new_end = new_start + duration;
                    
                    // Créer l'occurrence
                    let mut occurrence = event.clone();
                    occurrence.start = new_start;
                    occurrence.end = new_end;
                    expanded_events.push(occurrence);
                    
                    // Passer à la prochaine occurrence
                    current_date = current_date + Duration::days(7 * interval as i64);
                }
            } else {
                // RRULE non supporté, ajouter juste l'événement original
                expanded_events.push(event);
            }
        } else {
            // Événement non récurrent, ajouter tel quel
            expanded_events.push(event);
        }
    }
    
    expanded_events
}

/// Vérifie si un slot chevauche avec un événement du calendrier
fn is_slot_conflicting(slot: &Slot, calendar_events: &[CalendarEvent]) -> bool {
    let slot_date = match NaiveDate::parse_from_str(&slot.date, "%Y-%m-%d") {
        Ok(date) => date,
        Err(_) => return false,
    };
    let slot_start = match NaiveTime::parse_from_str(&slot.start_time, "%H:%M") {
        Ok(time) => time,
        Err(_) => return false,
    };
    let slot_end = match NaiveTime::parse_from_str(&slot.end_time, "%H:%M") {
        Ok(time) => time,
        Err(_) => return false,
    };
    
    // Convertir le slot en DateTime (UTC)
    let slot_start_dt: DateTime<Utc> = Utc.from_utc_datetime(&slot_date.and_time(slot_start));
    let slot_end_dt: DateTime<Utc> = Utc.from_utc_datetime(&slot_date.and_time(slot_end));
    
    // Vérifier chaque événement du calendrier
    for event in calendar_events {
        // Vérifier s'il y a chevauchement entre le slot et l'événement
        if slot_start_dt < event.end && slot_end_dt > event.start {
            return true; // Conflit trouvé
        }
    }
    
    false
}

/// Filtre les slots pour ne retourner que ceux disponibles (non réservés ET sans conflit calendrier)
async fn filter_available_slots(slots: Vec<Slot>, state: &web::Data<AppState>) -> Vec<Slot> {
    // Récupérer les événements du calendrier
    let calendar_events = fetch_calendar_events().await;
    
    slots.into_iter()
        .filter(|slot| {
            // Vérifier si déjà réservé
            if state.is_booked(&slot.id) {
                return false;
            }
            
            // Vérifier s'il y a un conflit avec le calendrier
            if is_slot_conflicting(slot, &calendar_events) {
                return false;
            }
            
            true
        })
        .collect()
}

// ============================================================================
// Slots Logic
// ============================================================================

/// Jours d'ouverture du service après-vente
const OPEN_DAYS: [chrono::Weekday; 4] = [
    chrono::Weekday::Mon, // Lundi
    chrono::Weekday::Tue, // Mardi
    chrono::Weekday::Thu, // Jeudi
    chrono::Weekday::Fri, // Vendredi
];

/// Plage horaire du matin (9h30 - 12h00)
const MORNING_START: NaiveTime = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
const MORNING_END: NaiveTime = NaiveTime::from_hms_opt(12, 0, 0).unwrap();

/// Plage horaire de l'après-midi (14h00 - 16h00)
const AFTERNOON_START: NaiveTime = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
const AFTERNOON_END: NaiveTime = NaiveTime::from_hms_opt(16, 0, 0).unwrap();

/// Durée d'un slot en minutes
const SLOT_DURATION_MINUTES: i64 = 30;

/// Minimum days in advance for booking
const MIN_BOOKING_DAYS: i64 = 2;

/// Génère tous les slots disponibles pour une date donnée
fn generate_slots_for_date(date: NaiveDate, base_visio_link: &str) -> Vec<Slot> {
    if !OPEN_DAYS.contains(&date.weekday()) {
        return Vec::new();
    }

    let mut slots = Vec::new();

    // Générer les slots du matin
    let mut current_time = MORNING_START;
    while current_time < MORNING_END {
        let visio_link = format!("{}/{}/{}", base_visio_link, date.format("%Y-%m-%d"), current_time.format("%H%M"));
        slots.push(Slot::new(date, current_time, visio_link));
        current_time = current_time + Duration::minutes(SLOT_DURATION_MINUTES);
    }

    // Générer les slots de l'après-midi
    let mut current_time = AFTERNOON_START;
    while current_time < AFTERNOON_END {
        let visio_link = format!("{}/{}/{}", base_visio_link, date.format("%Y-%m-%d"), current_time.format("%H%M"));
        slots.push(Slot::new(date, current_time, visio_link));
        current_time = current_time + Duration::minutes(SLOT_DURATION_MINUTES);
    }

    slots
}

/// Génère tous les slots disponibles pour une plage de dates
fn generate_slots_for_range(start_date: NaiveDate, end_date: NaiveDate, base_visio_link: &str) -> Vec<Slot> {
    let mut all_slots = Vec::new();
    let mut current_date = start_date;

    while current_date <= end_date {
        let slots = generate_slots_for_date(current_date, base_visio_link);
        all_slots.extend(slots);
        current_date = current_date.succ_opt().unwrap();
    }

    all_slots
}

// ============================================================================
// KMeet API Integration
// ============================================================================

/// Structure pour créer une salle KMeet
#[derive(Debug, Serialize)]
struct KMeetRoomRequest {
    /// Nom de la réunion
    name: String,
    /// Date/heure de début (format ISO 8601)
    start: String,
    /// Date/heure de fin (format ISO 8601)
    end: String,
    /// Participants (optionnel)
    #[serde(skip_serializing_if = "Option::is_none")]
    participants: Option<Vec<String>>,
}

/// Structure de réponse de l'API KMeet pour une salle créée
#[derive(Debug, Deserialize)]
struct KMeetRoomResponse {
    /// ID de la salle
    id: Option<String>,
    /// URL de la salle
    url: Option<String>,
    /// URL de la réunion
    meeting_url: Option<String>,
    /// Message d'erreur si applicable
    error: Option<String>,
    /// Code d'erreur
    error_code: Option<String>,
}

/// Récupère le token API KMeet depuis les variables d'environnement
fn get_kmeet_api_token() -> Option<String> {
    std::env::var("KMEET_API_TOKEN").ok()
}

/// Crée une salle KMeet via l'API Infomaniak
/// Retourne l'URL de la réunion si succès
async fn create_kmeet_room(
    name: &str,
    start_datetime: &DateTime<Utc>,
    end_datetime: &DateTime<Utc>,
) -> Option<String> {
    let api_token = get_kmeet_api_token()?;
    let client = reqwest::Client::new();
    
    // Formater les dates en ISO 8601 avec timezone UTC
    let start_iso = start_datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let end_iso = end_datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    
    // Construire la requête
    let request = KMeetRoomRequest {
        name: name.to_string(),
        start: start_iso,
        end: end_iso,
        participants: None, // Pas de participants spécifiés pour l'instant
    };
    
    // Appeler l'API Infomaniak KMeet
    let response = match client
        .post("https://api.infomaniak.com/1/kmeet/rooms")
        .header("Authorization", format!("Bearer {}", api_token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("Failed to call KMeet API: {}", e);
            return None;
        }
    };
    
    // Vérifier le status HTTP
    if !response.status().is_success() {
        eprintln!(
            "KMeet API error: HTTP {} - {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
        return None;
    }
    
    // Parser la réponse
    match response.json::<KMeetRoomResponse>().await {
        Ok(room_response) => {
            // Logs de debug
            if let Some(ref id) = room_response.id {
                eprintln!("KMeet room created with ID: {}", id);
            }
            if let Some(ref err) = room_response.error {
                eprintln!("KMeet API error: {}", err);
            }
            if let Some(ref err_code) = room_response.error_code {
                eprintln!("KMeet API error code: {}", err_code);
            }
            
            // Retourner l'URL de la réunion (privilégier meeting_url, puis url)
            room_response.meeting_url.or(room_response.url)
        }
        Err(e) => {
            eprintln!("Failed to parse KMeet response: {}", e);
            None
        }
    }
}

// ============================================================================
// Configuration
// ============================================================================

/// Récupère l'URL de base pour les liens visio depuis les variables d'environnement
fn get_visio_base_url() -> String {
    std::env::var("VISIO_BASE_URL")
        .unwrap_or_else(|_| "https://meet.kmeet.infomaniak.com/sav".to_string())
}

/// Récupère l'URL du calendrier depuis les variables d'environnement
fn get_kcalendar_url() -> Option<String> {
    std::env::var("KCALENDAR_URL").ok()
}

// ============================================================================
// API Endpoints
// ============================================================================

/// Réponse pour les détails d'un slot réservé
#[derive(Debug, Serialize)]
struct SlotBookingDetails {
    /// ID du slot
    slot_id: String,
    /// Statut de réservation
    booked: bool,
    /// URL KMeet si disponible
    kmeet_url: Option<String>,
}

/// Endpoint de santé
#[get("/api/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "message": "SAV Server is running"
    }))
}

/// Récupère les slots DISPONIBLES pour une date spécifique (format: YYYY-MM-DD)
#[get("/api/slots/{date}")]
async fn get_slots_by_date(
    date_str: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<impl Responder> {
    let date_str = date_str.into_inner();
    match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
        Ok(date) => {
            let all_slots = generate_slots_for_date(date, &get_visio_base_url());
            let available_slots = filter_available_slots(all_slots, &state).await;
            Ok(HttpResponse::Ok().json(json!({
                "date": date_str,
                "count": available_slots.len(),
                "slots": available_slots
            })))
        }
        Err(_) => {
            Ok(HttpResponse::BadRequest().json(json!({
                "error": "Invalid date format. Use YYYY-MM-DD"
            })))
        }
    }
}

/// Récupère les slots DISPONIBLES pour une plage de dates (query params: start, end)
#[get("/api/slots")]
async fn get_slots_range(
    query: web::Query<SlotsQueryParams>,
    state: web::Data<AppState>,
) -> Result<impl Responder> {
    let SlotsQueryParams { start: start_str, end: end_str } = query.into_inner();
    
    // Parse start date
    let start_date = if let Some(s) = start_str {
        match NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            Ok(date) => date,
            Err(_) => {
                return Ok(HttpResponse::BadRequest().json(json!({
                    "error": "Invalid start date format. Use YYYY-MM-DD"
                })));
            }
        }
    } else {
        // Par défaut, dans 2 jours (car réservation minimum 2 jours à l'avance)
        Utc::now().date_naive() + Duration::days(MIN_BOOKING_DAYS)
    };

    // Parse end date
    let end_date = if let Some(s) = end_str {
        match NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            Ok(date) => date,
            Err(_) => {
                return Ok(HttpResponse::BadRequest().json(json!({
                    "error": "Invalid end date format. Use YYYY-MM-DD"
                })));
            }
        }
    } else {
        // Par défaut, 7 jours après start_date
        start_date + Duration::days(7)
    };

    if start_date > end_date {
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "Start date must be before or equal to end date"
        })));
    }

    let all_slots = generate_slots_for_range(start_date, end_date, &get_visio_base_url());
    let available_slots = filter_available_slots(all_slots, &state).await;
    
    Ok(HttpResponse::Ok().json(json!({
        "start_date": start_date.format("%Y-%m-%d").to_string(),
        "end_date": end_date.format("%Y-%m-%d").to_string(),
        "count": available_slots.len(),
        "slots": available_slots
    })))
}

/// Requête pour réserver un slot (optionnellement avec un nom de client)
#[derive(Debug, Deserialize)]
struct BookSlotRequest {
    /// Nom du client (optionnel)
    #[serde(default)]
    customer_name: Option<String>,
    /// Email du client (optionnel)
    #[serde(default)]
    customer_email: Option<String>,
}

/// Réponse de réservation avec l'URL KMeet
#[derive(Debug, Serialize)]
struct BookSlotResponse {
    /// Statut de la réservation
    status: String,
    /// ID du slot réservé
    slot_id: String,
    /// URL KMeet de la réunion (si disponible)
    kmeet_url: Option<String>,
    /// Message
    message: String,
}

/// Récupère les détails d'un slot (y compris l'URL KMeet si réservé)
#[get("/api/slots/{slot_id}/booking")]
async fn get_slot_booking_details(
    slot_id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<impl Responder> {
    let slot_id = slot_id.into_inner();
    
    let booked = state.is_booked(&slot_id);
    let kmeet_url = if booked {
        state.get_kmeet_url(&slot_id)
    } else {
        None
    };
    
    let response = SlotBookingDetails {
        slot_id: slot_id.clone(),
        booked,
        kmeet_url,
    };
    
    Ok(HttpResponse::Ok().json(response))
}

/// Réserve un slot spécifique
#[post("/api/slots/{slot_id}/book")]
async fn book_slot(
    slot_id: web::Path<String>,
    state: web::Data<AppState>,
    request: web::Json<BookSlotRequest>,
) -> Result<impl Responder> {
    let slot_id = slot_id.into_inner();
    let slot_id_clone = slot_id.clone();
    
    // Extraire la date et l'heure du slot depuis l'ID (format: YYYYMMDD-HHMM)
    let date_part = &slot_id[..8];
    let time_part = &slot_id[9..];
    
    let slot_date = match NaiveDate::parse_from_str(date_part, "%Y%m%d") {
        Ok(date) => date,
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(json!({
                "error": "Invalid slot ID format. Expected YYYYMMDD-HHMM"
            })));
        }
    };
    
    let slot_time = match NaiveTime::parse_from_str(time_part, "%H%M") {
        Ok(time) => time,
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(json!({
                "error": "Invalid time format in slot ID"
            })));
        }
    };
    
    // Vérifier si le slot peut être réservé
    if !state.can_book(&slot_id, &slot_date.format("%Y-%m-%d").to_string()) {
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "Slot cannot be booked. It may already be booked or the date is too close (minimum 2 days in advance)"
        })));
    }
    
    // Créer le DateTime UTC pour la salle KMeet
    let start_datetime = Utc.from_utc_datetime(&slot_date.and_time(slot_time));
    let end_datetime = start_datetime + Duration::minutes(SLOT_DURATION_MINUTES);
    
    // Générer un nom pour la réunion
    let meeting_name = match (&request.customer_name, &request.customer_email) {
        (Some(name), Some(email)) => format!("SAV - {} ({})", name, email),
        (Some(name), None) => format!("SAV - {}", name),
        (None, Some(email)) => format!("SAV - {}", email),
        (None, None) => format!("SAV - {}", slot_date.format("%Y-%m-%d")),
    };
    
    // Log des informations de réservation
    eprintln!("Booking slot {} for {} at {}", slot_id, meeting_name, start_datetime);
    
    // Appeler l'API KMeet pour créer la salle
    let kmeet_url = create_kmeet_room(&meeting_name, &start_datetime, &end_datetime).await;
    
    // Réserver le slot (même si KMeet échoue, on réserve localement)
    if state.book_slot(slot_id, kmeet_url.clone()) {
        let response = BookSlotResponse {
            status: "booked".to_string(),
            slot_id: slot_id_clone,
            kmeet_url,
            message: "Slot successfully booked".to_string(),
        };
        Ok(HttpResponse::Ok().json(response))
    } else {
        Ok(HttpResponse::Conflict().json(json!({
            "error": "Failed to book slot"
        })))
    }
}

/// Endpoint racine avec la documentation
#[get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "name": "SAV Appointment Server",
        "version": "0.4.0",
        "description": "API for managing SAV appointment slots with Infomaniak KMeet integration",
        "endpoints": {
            "/api/health": "GET - Health check",
            "/api/slots": "GET - Get AVAILABLE slots for date range (query: start, end)",
            "/api/slots/{date}": "GET - Get AVAILABLE slots for specific date (YYYY-MM-DD)",
            "/api/slots/{slot_id}/booking": "GET - Get booking details for a specific slot (includes KMeet URL)",
            "/api/slots/{slot_id}/book": "POST - Book a specific slot (minimum 2 days in advance). Body: {customer_name?, customer_email?}"
        },
        "slot_duration": "30 minutes",
        "opening_hours": {
            "days": ["Monday", "Tuesday", "Thursday", "Friday"],
            "morning": "09:30 - 12:00",
            "afternoon": "14:00 - 16:00"
        },
        "booking_rules": {
            "minimum_advance_days": 2
        },
        "features": {
            "kmeet_integration": get_kmeet_api_token().is_some()
        },
        "configuration": {
            "kcalendar_url": get_kcalendar_url(),
            "visio_base_url": get_visio_base_url(),
            "kmeet_api_configured": get_kmeet_api_token().is_some()
        }
    }))
}

// ============================================================================
// Main
// ============================================================================

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Charger les variables d'environnement depuis le fichier .env
    dotenv().ok();
    
    println!("SAV Server starting on http://localhost:8080");
    
    // Afficher la configuration chargée
    println!("KCalendar URL: {:?}", get_kcalendar_url());
    println!("Visio Base URL: {}", get_visio_base_url());
    println!("KMeet API Token: {}", if get_kmeet_api_token().is_some() { "Configured" } else { "Not configured" });
    
    // Créer l'état partagé
    let state = web::Data::new(AppState::new());
    
    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(index)
            .service(health)
            .service(get_slots_by_date)
            .service(get_slots_range)
            .service(get_slot_booking_details)
            .service(book_slot)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_slot_creation() {
        let date = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap();
        let start_time = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        let slot = Slot::new(date, start_time, "https://meet.kmeet.test/abc".to_string());

        assert_eq!(slot.date, "2025-01-20");
        assert_eq!(slot.start_time, "09:30");
        assert_eq!(slot.end_time, "10:00");
        assert_eq!(slot.visio_link, "https://meet.kmeet.test/abc");
        assert_eq!(slot.id, "20250120-0930");
        assert_eq!(slot.booked, false);
    }

    #[test]
    fn test_generate_slots_for_monday() {
        // Lundi 20 janvier 2025
        let date = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap();
        let slots = generate_slots_for_date(date, "https://meet.kmeet.test");

        assert_eq!(slots.len(), 9);
        assert_eq!(slots[0].start_time, "09:30");
        assert_eq!(slots[0].end_time, "10:00");
        assert_eq!(slots[8].start_time, "15:30");
        assert_eq!(slots[8].end_time, "16:00");
    }

    #[test]
    fn test_generate_slots_for_saturday() {
        // Samedi 18 janvier 2025 (fermé)
        let date = NaiveDate::from_ymd_opt(2025, 1, 18).unwrap();
        let slots = generate_slots_for_date(date, "https://meet.kmeet.test");

        assert_eq!(slots.len(), 0);
    }

    #[test]
    fn test_generate_slots_for_wednesday() {
        // Mercredi 22 janvier 2025 (fermé)
        let date = NaiveDate::from_ymd_opt(2025, 1, 22).unwrap();
        let slots = generate_slots_for_date(date, "https://meet.kmeet.test");

        assert_eq!(slots.len(), 0);
    }

    #[test]
    fn test_generate_slots_for_range() {
        let start = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap(); // Lundi
        let end = NaiveDate::from_ymd_opt(2025, 1, 24).unwrap(); // Vendredi
        let slots = generate_slots_for_range(start, end, "https://meet.kmeet.test");

        assert_eq!(slots.len(), 36); // 4 * 9 = 36
    }

    #[test]
    fn test_visio_link_format() {
        let date = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap();
        let slots = generate_slots_for_date(date, "https://meet.kmeet.test");

        assert!(slots[0].visio_link.contains("2025-01-20"));
        assert!(slots[0].visio_link.contains("0930"));
    }

    #[test]
    fn test_parse_ics_datetime() {
        // Test avec format UTC: 20260912T140000Z
        let dt = parse_ics_datetime("20260912T140000Z");
        assert!(dt.is_some());
        
        // Test avec format local: 20260912T140000
        let dt = parse_ics_datetime("20260912T140000");
        assert!(dt.is_some());
    }

    #[test]
    fn test_app_state_book_slot() {
        let state = AppState::new();
        
        assert!(!state.is_booked("20250120-0930"));
        let kmeet_url = Some("https://meet.kmeet.test/room123".to_string());
        state.book_slot("20250120-0930".to_string(), kmeet_url.clone());
        assert!(state.is_booked("20250120-0930"));
        assert_eq!(state.get_kmeet_url("20250120-0930"), kmeet_url);
    }

    #[tokio::test]
    async fn test_filter_available_slots() {
        let state = AppState::new();
        
        state.book_slot("20250120-0930".to_string(), None);
        
        let date = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap();
        let all_slots = generate_slots_for_date(date, "https://meet.kmeet.test");
        let available = filter_available_slots(all_slots, &web::Data::new(state)).await;
        
        assert_eq!(available.len(), 8);
        assert!(!available.iter().any(|s| s.id == "20250120-0930"));
    }

    #[test]
    fn test_slot_booking_details() {
        let state = AppState::new();
        
        // Slot non réservé
        assert!(!state.is_booked("20250120-0930"));
        assert_eq!(state.get_kmeet_url("20250120-0930"), None);
        
        // Réserver le slot avec une URL KMeet
        let kmeet_url = Some("https://meet.kmeet.test/room123".to_string());
        state.book_slot("20250120-0930".to_string(), kmeet_url.clone());
        
        // Vérifier que le slot est réservé
        assert!(state.is_booked("20250120-0930"));
        assert_eq!(state.get_kmeet_url("20250120-0930"), kmeet_url);
    }
}
