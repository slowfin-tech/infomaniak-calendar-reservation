use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Datelike, Duration};
use reqwest::Method;
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
/// CALDAV_LOGIN est déjà nettoyé (sans le domaine)
fn get_caldav_config() -> Option<(String, String, String)> {
    let url = std::env::var("CALDAV_URL").ok()?;
    let login = std::env::var("CALDAV_LOGIN").ok()?;
    let password = std::env::var("CALDAV_PASSWORD").ok()?;
    
    Some((url, login, password))
}

/// Construit l'URL du calendrier CalDAV à partir des informations disponibles
/// Priorité: CALDAV_CALENDAR_URI > CALDAV_LOGIN
fn build_caldav_calendar_url() -> Option<String> {
    // Essayer CALDAV_CALENDAR_URI en premier (format: /calendars/ND04237/f28b6a18-5cdd-410b-9cc4-ae95932ff536/)
    if let Ok(calendar_uri) = std::env::var("CALDAV_CALENDAR_URI") {
        let caldav_url = std::env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".to_string());
        let caldav_base = caldav_url.trim_end_matches('/');
        let uri_clean = calendar_uri.trim_start_matches('/');
        return Some(format!("{}/{}", caldav_base, uri_clean));
    }
    
    // Sinon, utiliser une URL par défaut avec CALDAV_LOGIN
    let login = std::env::var("CALDAV_LOGIN").ok()?;
    let caldav_url = std::env::var("CALDAV_URL").unwrap_or_else(|_| "https://sync.infomaniak.com/".to_string());
    let caldav_base = caldav_url.trim_end_matches('/');
    
    // Pour Infomaniak, chaque utilisateur a un calendrier par défaut
    Some(format!("{}/calendars/{}/default/", caldav_base, login))
}

/// Récupère les événements CalDAV en utilisant le protocole REPORT
/// Utilise une requête CalDAV calendar-query pour récupérer les VEVENT
/// Optionnellement filtre par plage de dates côté serveur
async fn fetch_caldav_events(start_date: Option<DateTime<Utc>>, end_date: Option<DateTime<Utc>>) -> Option<String> {
    let calendar_url = build_caldav_calendar_url()?;
    let (_, login, password) = get_caldav_config()?;
    
    let client = reqwest::Client::new();
    
    // Construire la requête XML avec une plage de dates optionnelle
    let time_range = if let (Some(start), Some(end)) = (start_date, end_date) {
        format!("<C:time-range start=\"{}Z\" end=\"{}Z\"/>", 
            start.format("%Y%m%dT%H%M%S"), 
            end.format("%Y%m%dT%H%M%S"))
    } else {
        // Par défaut, récupérer tout depuis 1970
        "<C:time-range start=\"19700101T000000Z\"/>".to_string()
    };
    
    // Requête REPORT pour récupérer les événements du calendrier
    // Utilise calendar-query pour obtenir les VEVENT
    let report_xml = format!(r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav">
    <D:prop xmlns:D="DAV:">
        <C:calendar-data/>
    </D:prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                {}
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#, time_range);
    
    // Créer une méthode REPORT personnalisée
    let method = Method::from_bytes(b"REPORT").unwrap();
    
    let response = match client
        .request(method, &calendar_url)
        .header("Content-Type", "application/xml; charset=utf-8")
        .header("Depth", "1")
        .basic_auth(&login, Some(&password))
        .body(report_xml.to_string())
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("Failed to send CalDAV REPORT request: {}", e);
            return None;
        }
    };
    
    if !response.status().is_success() {
        eprintln!("CalDAV REPORT failed: HTTP {}", response.status());
        // Afficher le corps de la réponse pour le debug
        if let Ok(body) = response.text().await {
            eprintln!("Response body: {}", body);
        }
        return None;
    }
    
    // Lire la réponse XML
    let xml_response = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            eprintln!("Failed to read CalDAV response: {}", e);
            return None;
        }
    };
    
    // Log pour debug: afficher la réponse CalDAV complète
    eprintln!("=== CalDAV Response (raw XML) ===");
    eprintln!("{}", xml_response);
    eprintln!("=== End of CalDAV Response ===");
    
    // Extraire le contenu ICS de la réponse XML
    // Chercher entre <C:calendar-data>, <cal:calendar-data>, etc.
    // La réponse peut contenir plusieurs calendar-data avec différents namespaces
    let mut ics_parts = Vec::new();
    
    // Essayer différents namespaces
    let markers = [
        ("<C:calendar-data>", "</C:calendar-data>"),
        ("<cal:calendar-data>", "</cal:calendar-data>"),
        ("<calendar-data>", "</calendar-data>"),
    ];
    
    for (start_marker, end_marker) in &markers {
        let mut current_remaining = &xml_response[..];
        while let Some(start_idx) = current_remaining.find(start_marker) {
            if let Some(end_idx) = current_remaining[start_idx + start_marker.len()..].find(end_marker) {
                let content_start = start_idx + start_marker.len();
                let content_end = content_start + end_idx;
                let content = &current_remaining[content_start..content_end];
                ics_parts.push(content);
                current_remaining = &current_remaining[content_end..];
            } else {
                break;
            }
        }
    }
    
    if ics_parts.is_empty() {
        eprintln!("No calendar-data found in CalDAV response");
        return None;
    }
    
    // Log pour debug: afficher le contenu ICS extrait
    eprintln!("=== Extracted ICS Content ===");
    for (i, part) in ics_parts.iter().enumerate() {
        eprintln!("ICS Part {}: {}", i + 1, part);
    }
    eprintln!("=== End of ICS Content ===");
    
    // Concatenner tous les contenus ICS
    Some(ics_parts.join(""))
}

/// Vérifie si une date est en heure d'été (CEST) pour Europe/Zurich
/// CEST: dernier dimanche de mars au dernier dimanche d'octobre (UTC+2)
/// CET: le reste de l'année (UTC+1)
fn is_dst_europe_zurich(date: NaiveDate) -> bool {
    let year = date.year();
    
    // Trouver le dernier dimanche de mars
    let mut march_last = NaiveDate::from_ymd_opt(year, 3, 31).unwrap();
    while march_last.weekday() != chrono::Weekday::Sun {
        march_last = march_last.pred_opt().unwrap();
    }
    
    // Trouver le dernier dimanche d'octobre
    let mut october_last = NaiveDate::from_ymd_opt(year, 10, 31).unwrap();
    while october_last.weekday() != chrono::Weekday::Sun {
        october_last = october_last.pred_opt().unwrap();
    }
    
    // Vérifier si la date est dans la période DST (CEST)
    date >= march_last && date < october_last
}

/// Obtient l'offset en heures pour un timezone donné
fn get_timezone_offset(tzid: &str, date: NaiveDateTime) -> i64 {
    match tzid {
        "Europe/Zurich" => {
            if is_dst_europe_zurich(date.date()) {
                // CEST: UTC+2
                2
            } else {
                // CET: UTC+1
                1
            }
        }
        _ => {
            // TZID inconnu, supposer UTC
            0
        }
    }
}

/// Parse une date/heure au format ICS avec ou sans paramètres
/// Exemples: 20250120T093000Z, 20250120T093000, DTSTART;TZID=Europe/Zurich:20260911T140000
fn parse_ics_datetime(dt_str: &str) -> Option<DateTime<Utc>> {
    // Extraire la valeur après le dernier : (pour gérer les paramètres comme TZID=...)
    let colon_idx = dt_str.rfind(':')?;
    let value = &dt_str[colon_idx + 1..];
    
    // Extraire le TZID s'il est présent dans la partie avant le :
    // Format: DTSTART;TZID=Europe/Zurich
    let prop_part = &dt_str[..colon_idx];
    let tzid = if let Some(tzid_start) = prop_part.find("TZID=") {
        let tzid_start_idx = tzid_start + 5;
        let tzid_end = prop_part[tzid_start_idx..]
            .find(';')
            .map(|i| tzid_start_idx + i)
            .unwrap_or(prop_part.len());
        Some(&prop_part[tzid_start_idx..tzid_end])
    } else {
        None
    };
    
    // Essayer de parser avec timezone UTC (Z)
    if value.ends_with('Z') {
        let naive_str = &value[..value.len()-1];
        let naive = NaiveDateTime::parse_from_str(naive_str, "%Y%m%dT%H%M%S").ok()?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    
    // Parser comme NaiveDateTime
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        // Appliquer l'offset du timezone si présent
        if let Some(tz) = tzid {
            let offset_hours = get_timezone_offset(tz, naive);
            // Le naive datetime est dans le timezone local, il faut le convertir en UTC
            // Si la date est à 14:00 Europe/Zurich (UTC+2), alors en UTC c'est 12:00
            // Donc on soustrait l'offset
            return Some(Utc.from_utc_datetime(&naive) - Duration::hours(offset_hours));
        } else {
            // Pas de timezone explicite, supposer UTC
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    
    // Parser comme date seule
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        let naive = date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        if let Some(tz) = tzid {
            let offset_hours = get_timezone_offset(tz, naive);
            return Some(Utc.from_utc_datetime(&naive) - Duration::hours(offset_hours));
        } else {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    
    None
}

/// Parse une date au format ICS (ex: 20250120)
fn parse_ics_date(date_str: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date_str, "%Y%m%d").ok()
}

/// Représente un événement ICS brut avec ses propriétés
struct RawIcsEvent {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    summary: String,
    rrule: Option<String>,
    until: Option<DateTime<Utc>>,
    count: Option<i32>,
}

/// Parse une règle de récurrence (RRULE) basique
/// Retourne une liste de dates d'occurrence
fn expand_rrule_events(base_event: &RawIcsEvent, until: DateTime<Utc>) -> Vec<(DateTime<Utc>, DateTime<Utc>, String)> {
    let mut occurrences = Vec::new();
    
    let rrule = match &base_event.rrule {
        Some(r) => r,
        None => return vec![(base_event.start, base_event.end, base_event.summary.clone())],
    };
    
    // Parser la RRULE
    // Format: FREQ=DAILY;INTERVAL=1;UNTIL=... ou FREQ=WEEKLY;BYDAY=MO,TU;INTERVAL=1
    let parts: Vec<&str> = rrule.split(';').collect();
    let mut freq = None;
    let mut interval = 1;
    let mut byday = None;
    let mut rrule_until = base_event.until;
    
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "FREQ" => freq = Some(value),
                "INTERVAL" => interval = value.parse().unwrap_or(1),
                "UNTIL" => {
                    // Parser la date UNTIL
                    if let Some(dt) = parse_ics_datetime(&format!("UNTIL:{}", value)) {
                        rrule_until = Some(dt);
                    }
                }
                "BYDAY" => byday = Some(value),
                _ => {}
            }
        }
    }
    
    // Si pas de FREQ, retourner juste l'événement de base
    let freq = match freq {
        Some(f) => f,
        None => return vec![(base_event.start, base_event.end, base_event.summary.clone())],
    };
    
    // Calculer la date de fin de l'expansion
    let expand_until = rrule_until.unwrap_or(until);
    
    match freq {
        "DAILY" => {
            // Générer une occurrence par jour
            let mut current_start = base_event.start;
            let mut current_end = base_event.end;
            let event_duration = base_event.end - base_event.start;
            
            while current_start <= expand_until {
                occurrences.push((current_start, current_end, base_event.summary.clone()));
                current_start = current_start + chrono::Duration::days(interval as i64);
                current_end = current_start + event_duration;
            }
        }
        "WEEKLY" => {
            // Générer une occurrence par semaine
            let event_duration = base_event.end - base_event.start;
            
            // Parser BYDAY (ex: MO,TU,WE,TH,FR)
            let weekdays: Vec<chrono::Weekday> = byday
                .map(|bd| {
                    bd.split(',')
                        .filter_map(|d| match d {
                            "MO" => Some(chrono::Weekday::Mon),
                            "TU" => Some(chrono::Weekday::Tue),
                            "WE" => Some(chrono::Weekday::Wed),
                            "TH" => Some(chrono::Weekday::Thu),
                            "FR" => Some(chrono::Weekday::Fri),
                            "SA" => Some(chrono::Weekday::Sat),
                            "SU" => Some(chrono::Weekday::Sun),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec![base_event.start.weekday()]);
            
            let mut current_start = base_event.start;
            
            // Trouver le premier jour qui match BYDAY
            while current_start <= expand_until {
                if weekdays.contains(&current_start.weekday()) {
                    let current_end = current_start + event_duration;
                    if current_start >= base_event.start {
                        occurrences.push((current_start, current_end, base_event.summary.clone()));
                    }
                }
                current_start = current_start + chrono::Duration::days(1);
            }
            
            // Si INTERVAL > 1, ne garder qu'une semaine sur INTERVAL
            if interval > 1 && !weekdays.is_empty() {
                let mut filtered = Vec::new();
                let mut week_counter = 0;
                for (i, &(start, end, ref summary)) in occurrences.iter().enumerate() {
                    if i > 0 && start.weekday() == occurrences[0].0.weekday() {
                        week_counter += 1;
                    }
                    if week_counter % interval == 0 {
                        filtered.push((start, end, summary.clone()));
                    }
                }
                occurrences = filtered;
            }
        }
        "MONTHLY" => {
            // Pour simplifier, générer une occurrence par mois à la même date
            let event_duration = base_event.end - base_event.start;
            let mut current_start = base_event.start;
            
            while current_start <= expand_until {
                occurrences.push((current_start, current_start + event_duration, base_event.summary.clone()));
                // Passer au mois suivant
                if current_start.day() < 28 {
                    current_start = current_start + chrono::Duration::days(30 - current_start.day() as i64);
                } else {
                    current_start = current_start + chrono::Duration::days(35 - current_start.day() as i64);
                }
                // Simplification: on ne gère pas parfaitement le dernier jour du mois
                current_start = current_start.with_day(1).unwrap_or(current_start) + chrono::Duration::days(1);
                if current_start.day() != base_event.start.day() && base_event.start.day() <= 28 {
                    current_start = current_start.with_day(base_event.start.day()).unwrap_or(current_start);
                }
            }
        }
        "YEARLY" => {
            // Générer une occurrence par an
            let event_duration = base_event.end - base_event.start;
            let mut current_start = base_event.start;
            
            while current_start <= expand_until {
                occurrences.push((current_start, current_start + event_duration, base_event.summary.clone()));
                current_start = current_start + chrono::Duration::days(365);
            }
        }
        _ => {
            // FREQ non supportée, retourner juste l'événement de base
            occurrences.push((base_event.start, base_event.end, base_event.summary.clone()));
        }
    }
    
    // Si COUNT est spécifié, limiter le nombre d'occurrences
    if let Some(count) = base_event.count {
        occurrences.truncate(count as usize);
    }
    
    occurrences
}

/// Parse un fichier ICS et extrait les événements (y compris récurrents)
/// Cette fonction parse manuellement les composants VEVENT
fn parse_ics_events(ics_content: &str) -> Vec<(DateTime<Utc>, DateTime<Utc>, String)> {
    let mut all_events = Vec::new();
    let mut lines = ics_content.lines();
    
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        
        // Détecter le début d'un événement
        if trimmed == "BEGIN:VEVENT" {
            let mut start_dt: Option<DateTime<Utc>> = None;
            let mut end_dt: Option<DateTime<Utc>> = None;
            let mut summary = "Unknown event".to_string();
            let mut rrule: Option<String> = None;
            let mut until: Option<DateTime<Utc>> = None;
            let mut count: Option<i32> = None;
            
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
                    let value = &line_trimmed[colon_idx+1..];
                    
                    match prop_name.as_str() {
                        "DTSTART" => {
                            if let Some(dt) = parse_ics_datetime(line_trimmed) {
                                start_dt = Some(dt);
                            } else if let Some(date) = parse_ics_date(value) {
                                // Date seule = toute la journée
                                start_dt = Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())));
                            }
                        }
                        "DTEND" => {
                            if let Some(dt) = parse_ics_datetime(line_trimmed) {
                                end_dt = Some(dt);
                            } else if let Some(date) = parse_ics_date(value) {
                                // Date seule = toute la journée (fin = lendemain à minuit)
                                end_dt = Some(Utc.from_utc_datetime(&date.succ_opt().unwrap_or(date).and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())));
                            }
                        }
                        "SUMMARY" => {
                            summary = value.to_string();
                        }
                        "RRULE" => {
                            rrule = Some(value.to_string());
                        }
                        "UNTIL" => {
                            if let Some(dt) = parse_ics_datetime(&format!("UNTIL:{}", value)) {
                                until = Some(dt);
                            }
                        }
                        "COUNT" => {
                            count = value.parse().ok();
                        }
                        _ => {}
                    }
                }
            }
            
            // Ajouter l'événement si on a les dates
            if let (Some(start), Some(end)) = (start_dt, end_dt) {
                let raw_event = RawIcsEvent {
                    start,
                    end,
                    summary,
                    rrule,
                    until,
                    count,
                };
                
                // Expander les événements récurrents
                // Utiliser une date loin dans le futur pour l'expansion (5 ans)
                let far_future = Utc::now() + chrono::Duration::days(365 * 5);
                let mut event_occurrences = expand_rrule_events(&raw_event, far_future);
                all_events.append(&mut event_occurrences);
            }
        }
    }
    
    all_events
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
/// Note: Cette fonction n'est plus appelée au démarrage, mais peut être utilisée pour synchroniser manuellement
async fn load_caldav_events_and_update_slots(state: &web::Data<AppState>) -> Vec<(DateTime<Utc>, DateTime<Utc>, String)> {
    // Récupérer les événements via CalDAV (pour les 90 prochains jours)
    let future_end = Utc::now() + Duration::days(90);
    if let Some(ics_content) = fetch_caldav_events(Some(Utc::now()), Some(future_end)).await {
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

/// Représente un événement du calendrier
/// Les dates start et end sont en UTC (format ISO 8601: YYYY-MM-DDTHH:MM:SSZ)
#[derive(Debug, Serialize)]
struct CalendarEvent {
    /// Titre/summary de l'événement
    summary: String,
    /// Date/heure de début en UTC (format ISO 8601: YYYY-MM-DDTHH:MM:SSZ)
    start: String,
    /// Date/heure de fin en UTC (format ISO 8601: YYYY-MM-DDTHH:MM:SSZ)
    end: String,
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
            "/api/slots/{slot_id}/book": "POST - Book a specific slot (minimum 2 days in advance). Body: {customer_name?, customer_email?}",
            "/api/calendar/events": "GET - Get calendar events for the current week"
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

/// Récupère les événements du calendrier pour la semaine en cours
/// Utilise le protocole CalDAV (REPORT request) pour récupérer les événements
/// puis parse le contenu iCalendar (ICS) retourné dans la réponse
#[get("/api/calendar/events")]
async fn get_calendar_events() -> Result<impl Responder> {
    // Calculer la date de début de la semaine (lundi) et fin (dimanche)
    let today = Utc::now();
    let today_date = today.date_naive();
    
    // Calculer le lundi de la semaine en cours
    let weekday = today_date.weekday();
    let days_to_monday = match weekday {
        chrono::Weekday::Mon => 0,
        chrono::Weekday::Tue => 1,
        chrono::Weekday::Wed => 2,
        chrono::Weekday::Thu => 3,
        chrono::Weekday::Fri => 4,
        chrono::Weekday::Sat => 5,
        chrono::Weekday::Sun => 6,
    };
    let monday = today_date - Duration::days(days_to_monday);
    let sunday = monday + Duration::days(6);
    
    // Convertir en DateTime<Utc> pour la plage de la requête CalDAV
    let monday_start = monday.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    let sunday_end = sunday.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    let monday_start_utc = Utc.from_utc_datetime(&monday_start);
    let sunday_end_utc = Utc.from_utc_datetime(&sunday_end);
    
    // Récupérer les événements pour la semaine (filtre côté serveur)
    let ics_content = match fetch_caldav_events(Some(monday_start_utc), Some(sunday_end_utc)).await {
        Some(content) => content,
        None => {
            return Ok(HttpResponse::ServiceUnavailable().json(json!({
                "error": "Failed to fetch CalDAV events",
                "status": "unavailable"
            })));
        }
    };
    
    // Parser les événements ICS (avec gestion des TZID comme Europe/Zurich)
    let events = parse_ics_events(&ics_content);
    
    // Log pour debug: afficher les événements parsés
    eprintln!("=== Parsed Events ({} total) ===", events.len());
    for (i, (start, end, summary)) in events.iter().enumerate() {
        eprintln!("Event {}: {} -> {} ({})", i + 1, start, end, summary);
    }
    eprintln!("=== End of Parsed Events ===");
    
    // Filtrer les événements de la semaine (filtre local au cas où)
    let week_events: Vec<CalendarEvent> = events
        .into_iter()
        .filter(|(start, end, _)| {
            // Un événement est dans la semaine s'il commence avant la fin de la semaine
            // et se termine après le début de la semaine
            *start < sunday_end_utc + Duration::seconds(1) && *end > monday_start_utc
        })
        .map(|(start, end, summary)| {
            CalendarEvent {
                summary,
                start: start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                end: end.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            }
        })
        .collect();
    
    // Log pour debug: afficher les événements de la semaine
    eprintln!("=== Week Events ({} filtered) ===", week_events.len());
    for (i, event) in week_events.iter().enumerate() {
        eprintln!("Week Event {}: {} -> {} ({})", i + 1, event.start, event.end, event.summary);
    }
    eprintln!("=== End of Week Events ===");
    
    Ok(HttpResponse::Ok().json(json!({
        "week_start": monday.format("%Y-%m-%d").to_string(),
        "week_end": sunday.format("%Y-%m-%d").to_string(),
        "events": week_events
    })))
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
    
    // Créer l'état partagé (sans chargement initial des événements CalDAV)
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
            .service(get_calendar_events)
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
