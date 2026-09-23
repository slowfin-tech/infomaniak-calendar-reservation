use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Datelike, Duration};
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
    /// Date du rendez-vous (format YYYY-MM-DD) - non inclus dans le JSON
    #[serde(skip_serializing)]
    date: String,
    /// Heure de début (format HH:MM)
    start_time: String,
    /// Heure de fin (format HH:MM)
    end_time: String,
    /// Statut de réservation
    booked: bool,
}

impl Slot {
    /// Crée un nouveau slot
    fn new(date: NaiveDate, start_time: NaiveTime) -> Self {
        let end_time = start_time + Duration::minutes(30);
        let id = format!("{}{}", date.format("%Y%m%d"), start_time.format("%H%M"));
        
        Slot {
            id,
            date: date.format("%Y-%m-%d").to_string(),
            start_time: start_time.format("%H:%M").to_string(),
            end_time: end_time.format("%H:%M").to_string(),
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
    /// Liste des événements CalDAV (start, end, summary)
    caldav_events: Mutex<Vec<(DateTime<Utc>, DateTime<Utc>, String)>>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            booked_slots: Mutex::new(HashMap::new()),
            caldav_events: Mutex::new(Vec::new()),
        }
    }
    
    /// Vérifie si un slot est réservé (via API ou CalDAV)
    fn is_booked(&self, slot_id: &str) -> bool {
        let booked = self.booked_slots.lock().unwrap();
        booked.get(slot_id).map(|(_, booked)| *booked).unwrap_or(false)
    }
    
    /// Vérifie si un slot est en conflit avec un événement CalDAV
    fn is_in_caldav_conflict(&self, date: NaiveDate, start_time: NaiveTime, end_time: NaiveTime) -> bool {
        let events = self.caldav_events.lock().unwrap();
        let slot_start_utc = Utc.from_utc_datetime(&date.and_time(start_time));
        let slot_end_utc = Utc.from_utc_datetime(&date.and_time(end_time));
        
        for (event_start, event_end, _) in events.iter() {
            // Vérifier si les plages se chevauchent
            if slot_start_utc < *event_end && slot_end_utc > *event_start {
                return true;
            }
        }
        
        false
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
    
    /// Charge les événements CalDAV
    fn load_caldav_events(&self, events: Vec<(DateTime<Utc>, DateTime<Utc>, String)>) {
        let mut caldav_events = self.caldav_events.lock().unwrap();
        *caldav_events = events;
    }
    
    /// Vérifie si un slot peut être réservé (minimum 2 jours à l'avance et pas en conflit CalDAV)
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
        if slot_naive < min_date {
            return false;
        }
        
        // Vérifier les conflits CalDAV
        // Extraire l'heure du slot depuis l'ID (format: YYYYMMDDHHMM)
        if slot_id.len() >= 12 {
            let time_part = &slot_id[8..12];
            if let Ok(start_time) = NaiveTime::parse_from_str(time_part, "%H%M") {
                let end_time = start_time + Duration::minutes(SLOT_DURATION_MINUTES);
                if self.is_in_caldav_conflict(slot_naive, start_time, end_time) {
                    return false;
                }
            }
        }
        
        true
    }
}

// ============================================================================
// Slots Status Filtering
// ============================================================================

/// Filtre les slots et met à jour le champ booked en fonction de l'état de réservation.
/// Un slot est marqué comme booked: true si il est réservé via l'API ou en conflit avec CalDAV.
/// (retourne TOUS les slots valides)
async fn filter_slots_with_status(slots: Vec<Slot>, state: &web::Data<AppState>) -> Vec<Slot> {
    slots.into_iter()
        .map(|mut slot| {
            // Mettre à jour le statut booked en fonction de l'état de réservation via l'API
            let api_booked = state.is_booked(&slot.id);
            
            // Vérifier aussi les conflits CalDAV
            let caldav_booked = if !api_booked {
                let date = NaiveDate::parse_from_str(&slot.date, "%Y-%m-%d").unwrap_or_default();
                let start_time = NaiveTime::parse_from_str(&slot.start_time, "%H:%M").unwrap_or_default();
                let end_time = NaiveTime::parse_from_str(&slot.end_time, "%H:%M").unwrap_or_default();
                state.is_in_caldav_conflict(date, start_time, end_time)
            } else {
                false
            };
            
            slot.booked = api_booked || caldav_booked;
            slot
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
fn generate_slots_for_date(date: NaiveDate) -> Vec<Slot> {
    if !OPEN_DAYS.contains(&date.weekday()) {
        return Vec::new();
    }

    let mut slots = Vec::new();

    // Générer les slots du matin
    let mut current_time = MORNING_START;
    while current_time < MORNING_END {
        slots.push(Slot::new(date, current_time));
        current_time = current_time + Duration::minutes(SLOT_DURATION_MINUTES);
    }

    // Générer les slots de l'après-midi
    let mut current_time = AFTERNOON_START;
    while current_time < AFTERNOON_END {
        slots.push(Slot::new(date, current_time));
        current_time = current_time + Duration::minutes(SLOT_DURATION_MINUTES);
    }

    slots
}

/// Génère tous les slots disponibles pour une plage de dates
fn generate_slots_for_range(start_date: NaiveDate, end_date: NaiveDate) -> Vec<Slot> {
    let mut all_slots = Vec::new();
    let mut current_date = start_date;

    while current_date <= end_date {
        let slots = generate_slots_for_date(current_date);
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

// ============================================================================
// CalDAV Integration
// ============================================================================

/// Récupère les variables d'environnement CalDAV
fn get_caldav_config() -> Option<(String, String, String)> {
    let url = std::env::var("CALDAV_URL").ok()?;
    let login = std::env::var("CALDAV_LOGIN").ok()?;
    let password = std::env::var("CALDAV_PASSWORD").ok()?;
    Some((url, login, password))
}

/// Récupère l'URL du calendrier KCalendar depuis les variables d'environnement
fn get_kcalendar_url() -> Option<String> {
    std::env::var("KCALENDAR_URL").ok()
}

/// Construit l'URL complète du calendrier CalDAV pour Infomaniak
/// Format: https://sync.infomaniak.com/calendars/{user_id}/{calendar_id}
fn build_caldav_calendar_url() -> Option<String> {
    let kcalendar_url = get_kcalendar_url()?;
    // Extraire l'ID du calendrier de l'URL KCalendar
    // Format: https://sync.infomaniak.com/calendars/ND04237/f28b6a18-5cdd-410b-9cc4-ae95932ff536?export
    let parts: Vec<&str> = kcalendar_url.split("/calendars/").collect();
    if parts.len() < 2 {
        return None;
    }
    let calendar_path = parts[1].split('?').next().unwrap_or("");
    
    let caldav_url = std::env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".to_string());
    
    // Nettoyer les slashes de fin
    let caldav_base = caldav_url.trim_end_matches('/');
    
    Some(format!("{}/calendars/{}", caldav_base, calendar_path))
}

/// Récupère le calendrier ICS depuis CalDAV ou KCalendar
/// Infomaniak supporte l'export ICS via l'URL KCalendar avec authentification
async fn fetch_ics_calendar() -> Option<String> {
    let client = reqwest::Client::new();
    
    // Essayer d'abord avec KCalendar URL (export ICS)
    if let Some(kcalendar_url) = get_kcalendar_url() {
        // Ajouter l'authentification si disponible
        if let Some((_, login, password)) = get_caldav_config() {
            let response = match client
                .get(&kcalendar_url)
                .basic_auth(&login, Some(&password))
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Failed to fetch KCalendar: {}", e);
                    return None;
                }
            };
            
            if response.status().is_success() {
                return response.text().await.ok();
            }
        } else {
            // Essayer sans authentification
            let response = match client
                .get(&kcalendar_url)
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Failed to fetch KCalendar: {}", e);
                    return None;
                }
            };
            
            if response.status().is_success() {
                return response.text().await.ok();
            }
        }
    }
    
    // Sinon, essayer de construire l'URL CalDAV avec ?export
    if let Some(calendar_url) = build_caldav_calendar_url() {
        if let Some((_, login, password)) = get_caldav_config() {
            // Ajouter ?export à l'URL
            let export_url = if calendar_url.contains('?') {
                format!("{}&export", calendar_url)
            } else {
                format!("{}/?export", calendar_url)
            };
            
            let response = match client
                .get(&export_url)
                .basic_auth(&login, Some(&password))
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Failed to fetch CalDAV export: {}", e);
                    return None;
                }
            };
            
            if response.status().is_success() {
                return response.text().await.ok();
            }
        }
    }
    
    None
}

/// Parse une date/heure au format ICS avec ou sans paramètres
/// Exemples: 20250120T093000Z, 20250120T093000, 20250120T093000 (avec TZID=Europe/Zurich)
fn parse_ics_datetime(dt_str: &str) -> Option<DateTime<Utc>> {
    // Extraire la valeur après le :
    let value = if let Some(idx) = dt_str.find(':') {
        &dt_str[idx+1..]
    } else {
        dt_str
    };
    
    // Essayer de parser avec timezone UTC (Z)
    if value.ends_with('Z') {
        let naive_str = &value[..value.len()-1];
        let naive = NaiveDateTime::parse_from_str(naive_str, "%Y%m%dT%H%M%S").ok()?;
        Some(Utc.from_utc_datetime(&naive))
    } else if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        // Pas de timezone explicite, supposer UTC
        // Note: On devrait idéalement utiliser le TZID du VTIMEZONE, mais pour simplifier on convertit en UTC
        Some(Utc.from_utc_datetime(&naive))
    } else if let Ok(date) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        // Date seule
        Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())))
    } else {
        None
    }
}

/// Parse une date au format ICS (ex: 20250120)
fn parse_ics_date(date_str: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date_str, "%Y%m%d").ok()
}

/// Parse un fichier ICS et extrait les événements
/// Cette fonction parse manuellement les composants VEVENT
fn parse_ics_events(ics_content: &str) -> Vec<(DateTime<Utc>, DateTime<Utc>, String)> {
    let mut events = Vec::new();
    let mut lines = ics_content.lines();
    
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        
        // Détecter le début d'un événement
        if trimmed == "BEGIN:VEVENT" {
            let mut start_dt: Option<DateTime<Utc>> = None;
            let mut end_dt: Option<DateTime<Utc>> = None;
            let mut duration: Option<chrono::Duration> = None;
            let mut summary = "Unknown event".to_string();
            
            // Lire les propriétés de l'événement
            while let Some(line) = lines.next() {
                let line_trimmed = line.trim();
                
                // Fin de l'événement
                if line_trimmed == "END:VEVENT" {
                    break;
                }
                
                // Extraire la clé et la valeur (gérer les paramètres comme DTSTART;TZID=...:)
                // Format: PROPNAME[;param1=value1;param2=value2]:value
                if let Some(colon_idx) = line_trimmed.find(':') {
                    // Extraire le nom de la propriété (avant le premier ; ou :)
                    let prop_name_end = line_trimmed.find(';').unwrap_or(colon_idx);
                    let prop_name = &line_trimmed[..prop_name_end].to_uppercase();
                    
                    match prop_name.as_str() {
                        "DTSTART" => {
                            if let Some(dt) = parse_ics_datetime(line_trimmed) {
                                start_dt = Some(dt);
                            } else if let Some(date) = parse_ics_date(&line_trimmed[colon_idx+1..]) {
                                // Date seule = toute la journée
                                start_dt = Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())));
                            }
                        }
                        "DTEND" => {
                            if let Some(dt) = parse_ics_datetime(line_trimmed) {
                                end_dt = Some(dt);
                            } else if let Some(date) = parse_ics_date(&line_trimmed[colon_idx+1..]) {
                                // Date seule = toute la journée (fin = lendemain à minuit)
                                end_dt = Some(Utc.from_utc_datetime(&date.succ_opt().unwrap_or(date).and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())));
                            }
                        }
                        "DURATION" => {
                            duration = parse_duration(&line_trimmed[colon_idx+1..]);
                        }
                        "SUMMARY" => {
                            summary = line_trimmed[colon_idx+1..].to_string();
                        }
                        _ => {}
                    }
                }
            }
            
            // Calculer end_dt si durée est présente
            if start_dt.is_some() && duration.is_some() && end_dt.is_none() {
                end_dt = Some(start_dt.unwrap() + duration.unwrap());
            }
            
            // Ajouter l'événement si on a les dates
            if let (Some(start), Some(end)) = (start_dt, end_dt) {
                events.push((start, end, summary.clone()));
            }
        }
    }
    
    events
}

/// Parse une durée au format ISO 8601 (ex: PT30M, PT1H30M)
fn parse_duration(dur_str: &str) -> Option<chrono::Duration> {
    if !dur_str.starts_with("PT") {
        return None;
    }
    
    let dur = &dur_str[2..];
    let mut seconds = 0;
    let mut current_num: Option<i64> = None;
    
    for c in dur.chars() {
        if c.is_ascii_digit() {
            current_num = Some(current_num.unwrap_or(0) * 10 + c.to_digit(10)? as i64);
        } else {
            if let Some(num) = current_num {
                match c {
                    'H' => seconds += num * 3600,
                    'M' => seconds += num * 60,
                    'S' => seconds += num,
                    _ => {}
                }
                current_num = None;
            }
        }
    }
    
    Some(chrono::Duration::seconds(seconds))
}

/// Vérifie si un slot est en conflit avec un événement calendrier
fn is_slot_in_conflict(slot_date: NaiveDate, slot_start: NaiveTime, slot_end: NaiveTime, events: &[(DateTime<Utc>, DateTime<Utc>, String)]) -> bool {
    let slot_start_utc = Utc.from_utc_datetime(&slot_date.and_time(slot_start));
    let slot_end_utc = Utc.from_utc_datetime(&slot_date.and_time(slot_end));
    
    for (event_start, event_end, _) in events {
        // Vérifier si les plages se chevauchent
        if slot_start_utc < *event_end && slot_end_utc > *event_start {
            return true;
        }
    }
    
    false
}

/// Charge les événements CalDAV et met à jour l'état des slots réservés
async fn load_caldav_events_and_update_slots(state: &web::Data<AppState>) -> Vec<(DateTime<Utc>, DateTime<Utc>, String)> {
    // Récupérer le calendrier ICS
    if let Some(ics_content) = fetch_ics_calendar().await {
        let events = parse_ics_events(&ics_content);
        
        eprintln!("Loaded {} events from CalDAV", events.len());
        
        // Marquer les slots en conflit comme réservés
        let mut booked = state.booked_slots.lock().unwrap();
        
        // Générer des slots pour les prochains mois et vérifier les conflits
        let today = Utc::now().date_naive();
        let future_end = today + Duration::days(90); // 3 mois à l'avance
        
        let mut current_date = today;
        while current_date <= future_end {
            if OPEN_DAYS.contains(&current_date.weekday()) {
                let slots = generate_slots_for_date(current_date);
                for slot in slots {
                    let slot_id = slot.id.clone();
                    let start_time = NaiveTime::parse_from_str(&slot.start_time, "%H:%M").unwrap();
                    let end_time = NaiveTime::parse_from_str(&slot.end_time, "%H:%M").unwrap();
                    
                    if is_slot_in_conflict(current_date, start_time, end_time, &events) {
                        // Marquer comme réservé (sans URL KMeet)
                        if !booked.contains_key(&slot_id) {
                            booked.insert(slot_id.clone(), (None, true));
                            eprintln!("Marked slot {} as booked (CalDAV conflict)", slot_id);
                        }
                    }
                }
            }
            current_date = current_date.succ_opt().unwrap();
        }
        
        return events;
    }
    
    Vec::new()
}

// ============================================================================
// API Response Structures
// ============================================================================

/// Réponse pour un jour avec ses slots
#[derive(Debug, Serialize)]
struct DaySlots {
    /// Date du jour (format YYYY-MM-DD)
    date: String,
    /// Liste des slots disponibles pour ce jour
    slots: Vec<Slot>,
}

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

// ============================================================================
// API Endpoints
// ============================================================================


/// Endpoint de santé
#[get("/api/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "message": "SAV Server is running"
    }))
}

/// Récupère TOUS les slots (y compris réservés) pour une date spécifique (format: YYYY-MM-DD)
#[get("/api/slots/{date}")]
async fn get_slots_by_date(
    date_str: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<impl Responder> {
    let date_str = date_str.into_inner();
    match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
        Ok(date) => {
            let all_slots = generate_slots_for_date(date);
            let slots_with_status = filter_slots_with_status(all_slots, &state).await;
            
            // Créer la réponse avec le nouveau format
            let response = DaySlots {
                date: date_str,
                slots: slots_with_status,
            };
            
            Ok(HttpResponse::Ok().json(json!({
                "days": [response]
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

    let all_slots = generate_slots_for_range(start_date, end_date);
    let slots_with_status = filter_slots_with_status(all_slots, &state).await;
    
    // Grouper les slots par date
    let mut slots_by_date: HashMap<String, Vec<Slot>> = HashMap::new();
    for slot in slots_with_status {
        slots_by_date.entry(slot.date.clone()).or_default().push(slot);
    }
    
    // Convertir en vecteur de DaySlots trié par date
    let mut days: Vec<DaySlots> = slots_by_date
        .into_iter()
        .map(|(date, slots)| DaySlots { date, slots })
        .collect();
    
    // Trier par date
    days.sort_by(|a, b| a.date.cmp(&b.date));
    
    Ok(HttpResponse::Ok().json(json!({
        "days": days
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
    
    // Extraire la date et l'heure du slot depuis l'ID (format: YYYYMMDDHHMM)
    if slot_id.len() != 12 {
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "Invalid slot ID format. Expected YYYYMMDDHHMM (12 digits)"
        })));
    }
    
    let date_part = &slot_id[..8];
    let time_part = &slot_id[8..12];
    
    let slot_date = match NaiveDate::parse_from_str(date_part, "%Y%m%d") {
        Ok(date) => date,
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(json!({
                "error": "Invalid date format in slot ID. Expected YYYYMMDDHHMM"
            })));
        }
    };
    
    let slot_time = match NaiveTime::parse_from_str(time_part, "%H%M") {
        Ok(time) => time,
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(json!({
                "error": "Invalid time format in slot ID. Expected YYYYMMDDHHMM"
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
            "/api/slots": "GET - Get ALL slots (including booked) for date range (query: start, end)",
            "/api/slots/{date}": "GET - Get ALL slots (including booked) for specific date (YYYY-MM-DD)",
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
    println!("KMeet API Token: {}", if get_kmeet_api_token().is_some() { "Configured" } else { "Not configured" });
    
    // Vérifier la configuration CalDAV
    if let Some((url, login, _)) = get_caldav_config() {
        println!("CalDAV configured: URL={}, Login={}", url, login);
    } else {
        println!("CalDAV: Not configured");
    }
    
    // Créer l'état partagé
    let state = web::Data::new(AppState::new());
    
    // Charger les événements CalDAV au démarrage
    let events = load_caldav_events_and_update_slots(&state).await;
    state.load_caldav_events(events);
    println!("CalDAV synchronization complete");
    
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
        let slot = Slot::new(date, start_time);

        assert_eq!(slot.date, "2025-01-20");
        assert_eq!(slot.start_time, "09:30");
        assert_eq!(slot.end_time, "10:00");
        assert_eq!(slot.id, "202501200930");
        assert_eq!(slot.booked, false);
    }

    #[test]
    fn test_generate_slots_for_monday() {
        // Lundi 20 janvier 2025
        let date = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap();
        let slots = generate_slots_for_date(date);

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
        let slots = generate_slots_for_date(date);

        assert_eq!(slots.len(), 0);
    }

    #[test]
    fn test_generate_slots_for_wednesday() {
        // Mercredi 22 janvier 2025 (fermé)
        let date = NaiveDate::from_ymd_opt(2025, 1, 22).unwrap();
        let slots = generate_slots_for_date(date);

        assert_eq!(slots.len(), 0);
    }

    #[test]
    fn test_generate_slots_for_range() {
        let start = NaiveDate::from_ymd_opt(2025, 1, 20).unwrap(); // Lundi
        let end = NaiveDate::from_ymd_opt(2025, 1, 24).unwrap(); // Vendredi
        let slots = generate_slots_for_range(start, end);

        assert_eq!(slots.len(), 36); // 4 * 9 = 36
    }

    #[test]
    fn test_app_state_book_slot() {
        let state = AppState::new();
        
        assert!(!state.is_booked("202501200930"));
        let kmeet_url = Some("https://meet.kmeet.test/room123".to_string());
        state.book_slot("202501200930".to_string(), kmeet_url.clone());
        assert!(state.is_booked("202501200930"));
        assert_eq!(state.get_kmeet_url("202501200930"), kmeet_url);
    }

    #[test]
    fn test_slot_booking_details() {
        let state = AppState::new();
        
        // Slot non réservé
        assert!(!state.is_booked("202501200930"));
        assert_eq!(state.get_kmeet_url("202501200930"), None);
        
        // Réserver le slot avec une URL KMeet
        let kmeet_url = Some("https://meet.kmeet.test/room123".to_string());
        state.book_slot("202501200930".to_string(), kmeet_url.clone());
        
        // Vérifier que le slot est réservé
        assert!(state.is_booked("202501200930"));
        assert_eq!(state.get_kmeet_url("202501200930"), kmeet_url);
    }
}
