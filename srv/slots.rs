//! Module for SAV slots: time windows derived from CalDAV events.
//!
//! A slot ID is formatted `YYYYMMDDHHMM` (start of the slot, Europe/Zurich
//! local time as returned by the calendar). Slot duration and daily periods
//! are configurable (see `SlotsConfig`): by default, 30-minute slots in
//! 10:00-12:00 and 14:00-16:00 every day. A slot overlapping any calendar
//! event is marked booked; all slots are listed grouped by date, with their
//! booked status.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::calendar::{get_sliding_week_raw_events, get_sliding_week_range, CalDavConfig};

/// Periode horaire d'une journee: ((h,m) debut, (h,m) fin).
type Period = ((u32, u32), (u32, u32));

#[derive(Debug, Serialize)]
pub struct Slot {
    pub id: String,
    pub start_at: String,
    pub booked: bool,
}

#[derive(Debug, Serialize)]
pub struct SlotsResponse {
    pub slots_by_date: BTreeMap<String, Vec<Slot>>,
}

/// Configuration des creneaux.
///
/// Variables d'environnement:
/// - `SAV_SLOT_DURATION`: duree des creneaux en minutes (defaut 30)
/// - `SAV_PERIODS`: JSON des periodes par jour de semaine. Cles: mon, tue,
///   wed, thu, fri, sat, sun. Chaque jour liste des periodes "HH:MM-HH:MM".
///   Exemple:
///   `SAV_PERIODS={"mon":["10:00-12:00","14:00-16:00"],"sat":["10:00-12:00"]}`
///
/// Si `SAV_PERIODS` est defini, il remplace la configuration par defaut
/// (tous les jours 10:00-12:00 et 14:00-16:00): les jours non listes sont
/// fermes (aucun creneau).
#[derive(Debug, Clone)]
pub struct SlotsConfig {
    /// Duree d'un creneau en minutes.
    pub duration_minutes: i64,
    /// Delai minimum (minutes) entre maintenant et le debut d'un creneau
    /// pour qu'il soit proposable et reservable.
    pub min_delay_minutes: i64,
    /// Periodes par jour de la semaine (0 = lundi ... 6 = dimanche).
    pub(crate) periods: [Vec<Period>; 7],
}

pub const DEFAULT_DURATION_MINUTES: i64 = 30;

impl SlotsConfig {
    /// Configuration depuis config.toml ([slots]).
    pub fn from_app_config(app: &crate::config::AppConfig) -> SlotsConfig {
        let mut config = SlotsConfig {
            duration_minutes: app
                .slots
                .duration_minutes
                .filter(|d| *d > 0)
                .unwrap_or(DEFAULT_DURATION_MINUTES),
            min_delay_minutes: app.slots.min_delay_minutes.filter(|d| *d >= 0).unwrap_or(0),
            periods: Self::default_periods_all(),
        };

        if let Some(spec) = &app.slots.periods {
            // La section remplace la config par defaut: jours non listes
            // = fermes.
            config.periods = std::array::from_fn(|_| Vec::new());
            for (day, periods) in spec {
                let Some(index) = weekday_index(day) else {
                    eprintln!("[slots.periods]: jour inconnu \"{day}\", ignore");
                    continue;
                };
                let mut parsed = Vec::new();
                for item in periods {
                    match parse_period(item) {
                        Some(period) => parsed.push(period),
                        None => eprintln!("[slots.periods]: periode invalide \"{item}\" pour {day}, ignoree"),
                    }
                }
                config.periods[index] = parsed;
            }
        }

        config
    }

    fn default_periods() -> Vec<Period> {
        vec![((10, 0), (12, 0)), ((14, 0), (16, 0))]
    }

    /// Periodes par defaut pour les 7 jours de la semaine.
    fn default_periods_all() -> [Vec<Period>; 7] {
        std::array::from_fn(|_| Self::default_periods())
    }

    /// Parse le JSON SAV_PERIODS: jours non listes = fermes.
    /// Debuts (heure, minute) des creneaux pour un jour de semaine donne.
    pub fn starts_for(&self, weekday: usize) -> Vec<(u32, u32)> {
        let mut starts = Vec::new();
        let duration = self.duration_minutes as u32;
        for ((start_h, start_m), (end_h, end_m)) in &self.periods[weekday] {
            let mut current = start_h * 60 + start_m;
            let end = end_h * 60 + end_m;
            while current + duration <= end {
                starts.push((current / 60, current % 60));
                current += duration;
            }
        }
        starts
    }

    /// Le couple (heure, minute) est-il un debut de creneau valide ce jour ?
    pub fn is_valid_start(&self, weekday: usize, hour: u32, minute: u32) -> bool {
        self.starts_for(weekday).contains(&(hour, minute))
    }
}

fn weekday_index(day: &str) -> Option<usize> {
    match day.to_ascii_lowercase().as_str() {
        "mon" => Some(0),
        "tue" => Some(1),
        "wed" => Some(2),
        "thu" => Some(3),
        "fri" => Some(4),
        "sat" => Some(5),
        "sun" => Some(6),
        _ => None,
    }
}

/// Parse une periode "HH:MM-HH:MM" (debut < fin requis).
fn parse_period(spec: &str) -> Option<Period> {
    let (start, end) = spec.trim().split_once('-')?;
    let parse_hm = |s: &str| -> Option<(u32, u32)> {
        let (h, m) = s.trim().split_once(':')?;
        let h: u32 = h.trim().parse().ok()?;
        let m: u32 = m.trim().parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some((h, m))
    };
    let start = parse_hm(start)?;
    let end = parse_hm(end)?;
    if start >= end {
        return None;
    }
    Some((start, end))
}

fn weekday_of(date: &NaiveDate) -> usize {
    date.weekday().num_days_from_monday() as usize
}

/// Un creneau est proposable/reservable seulement s'il debute apres
/// maintenant + delai minimum (couvre aussi les creneaux deja passes).
pub(crate) fn respects_min_delay(
    slot_start: NaiveDateTime,
    now: NaiveDateTime,
    min_delay_minutes: i64,
) -> bool {
    slot_start >= now + Duration::minutes(min_delay_minutes)
}

fn slot_id(date: NaiveDate, hour: u32, minute: u32) -> String {
    format!("{}{:02}{:02}", date.format("%Y%m%d"), hour, minute)
}

/// A slot is booked if a calendar event overlaps it (half-open intervals).
pub(crate) fn is_booked(
    events: &[crate::calendar::RawEvent],
    slot_start: NaiveDateTime,
    slot_end: NaiveDateTime,
) -> bool {
    events
        .iter()
        .any(|event| event.start < slot_end && event.end > slot_start)
}

/// Lists all slots for the sliding week grouped by date, with their booked status.
pub async fn get_week_slots(caldav: &CalDavConfig, slots: &SlotsConfig) -> SlotsResponse {
    let (range_start, range_end) = get_sliding_week_range();
    let events = get_sliding_week_raw_events(caldav).await;
    let now = Local::now().naive_local();

    let mut slots_by_date: BTreeMap<String, Vec<Slot>> = BTreeMap::new();
    let mut date = range_start;

    while date <= range_end {
        for (hour, minute) in slots.starts_for(weekday_of(&date)) {
            let slot_start = date.and_hms_opt(hour, minute, 0).unwrap();
            let slot_end = slot_start + Duration::minutes(slots.duration_minutes);

            // Un creneau deja passe ou trop proche (delai minimum) n'est
            // pas listable.
            if !respects_min_delay(slot_start, now, slots.min_delay_minutes) {
                continue;
            }

            let date_key = date.format("%Y-%m-%d").to_string();
            slots_by_date.entry(date_key).or_default().push(Slot {
                id: slot_id(date, hour, minute),
                start_at: format!("{:02}:{:02}", hour, minute),
                booked: is_booked(&events, slot_start, slot_end),
            });
        }
        date += Duration::days(1);
    }

    SlotsResponse { slots_by_date }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn config_with(duration_minutes: i64, periods: [Vec<Period>; 7]) -> SlotsConfig {
        SlotsConfig { duration_minutes, min_delay_minutes: 0, periods }
    }

    #[test]
    fn starts_defaut_30min_deux_fenetres() {
        let config = SlotsConfig {
            duration_minutes: 30,
            min_delay_minutes: 0,
            periods: SlotsConfig::default_periods_all(),
        };
        assert_eq!(
            config.starts_for(0),
            vec![
                (10, 0), (10, 30), (11, 0), (11, 30),
                (14, 0), (14, 30), (15, 0), (15, 30),
            ]
        );
    }

    #[test]
    fn duree_configuree_change_la_grille() {
        let config = config_with(60, SlotsConfig::default_periods_all());
        assert_eq!(
            config.starts_for(2),
            vec![(10, 0), (11, 0), (14, 0), (15, 0)]
        );
    }

    #[test]
    fn periodes_configurees_par_jour() {
        let mut periods: [Vec<Period>; 7] = std::array::from_fn(|_| Vec::new());
        periods[0] = vec![((9, 0), (11, 30))]; // lundi: une seule fenetre
        periods[5] = vec![((10, 0), (12, 0))]; // samedi
        let config = config_with(30, periods);

        // Lundi: 9h-11h30 en tranches de 30 min (11h30+30 depasse).
        assert_eq!(
            config.starts_for(0),
            vec![(9, 0), (9, 30), (10, 0), (10, 30), (11, 0)]
        );
        // Mardi ferme, samedi ouvert.
        assert!(config.starts_for(1).is_empty());
        assert_eq!(config.starts_for(5), vec![(10, 0), (10, 30), (11, 0), (11, 30)]);
        // Validite d'un debut de creneau.
        assert!(config.is_valid_start(0, 9, 30));
        assert!(!config.is_valid_start(0, 11, 30));
        assert!(!config.is_valid_start(1, 10, 0));
    }

    #[test]
    fn parse_period_valide_et_invalide() {
        assert_eq!(parse_period("10:00-12:00"), Some(((10, 0), (12, 0))));
        assert_eq!(parse_period(" 9:15 - 10h45 "), None); // format invalide
        assert_eq!(parse_period("12:00-10:00"), None); // fin avant debut
        assert_eq!(parse_period("24:00-25:00"), None); // heures invalides
    }

    #[test]
    fn weekday_index_cles() {
        assert_eq!(weekday_index("mon"), Some(0));
        assert_eq!(weekday_index("SUN"), Some(6));
        assert_eq!(weekday_index("lun"), None);
    }

    #[test]
    fn spec_periods_parse_les_jours_connus() {
        // from_app_config avec une section [slots.periods] partielle:
        // jours listes configures, jour inconnu ignore, autres fermes.
        let app = crate::config::AppConfig {
            slots: crate::config::SlotsConfigToml {
                duration_minutes: Some(30),
                min_delay_minutes: None,
                periods: Some(
                    [
                        ("mon".to_string(), vec!["10:00-12:00".to_string(), "14:00-16:00".to_string()]),
                        ("sat".to_string(), vec!["10:00-12:00".to_string()]),
                        ("bad".to_string(), vec!["09:00-10:00".to_string()]),
                    ]
                    .into_iter()
                    .collect(),
                ),
            },
            ..Default::default()
        };

        let config = SlotsConfig::from_app_config(&app);
        assert_eq!(config.periods[0].len(), 2);
        assert_eq!(config.periods[5].len(), 1);
        // Jour inconnu ignore; les autres jours restent fermes.
        assert!(config.periods[1].is_empty());
    }

    #[test]
    fn sans_section_periods_valeurs_par_defaut() {
        let app = crate::config::AppConfig::default();
        let config = SlotsConfig::from_app_config(&app);
        for weekday in 0..7 {
            assert_eq!(config.periods[weekday].len(), 2);
        }
    }

    #[test]
    fn slot_id_format() {
        let id = slot_id(NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(), 14, 30);
        assert_eq!(id, "202609281430");
    }

    #[test]
    fn booked_on_overlap_only() {
        use crate::calendar::RawEvent;
        let date = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
        let slot_start = date.and_hms_opt(11, 0, 0).unwrap();
        let slot_end = slot_start + Duration::minutes(30);

        let event = |h1: u32, m1: u32, h2: u32, m2: u32| RawEvent {
            summary: "RDV".to_string(),
            start: date.and_hms_opt(h1, m1, 0).unwrap(),
            end: date.and_hms_opt(h2, m2, 0).unwrap(),
            uid: Some("uid-1".to_string()),
        };

        // Evenement a cheval sur le debut du slot -> booked.
        let overlapping = vec![event(10, 45, 11, 15)];
        assert!(is_booked(&overlapping, slot_start, slot_end));

        // Evenement se terminant exactement au debut du slot -> pas booked.
        let adjacent = vec![event(10, 30, 11, 0)];
        assert!(!is_booked(&adjacent, slot_start, slot_end));
    }

    // Lundi 2026-10-05 -> index 0, dimanche 2026-10-11 -> index 6.
    #[test]
    fn weekday_of_date() {
        assert_eq!(
            weekday_of(&NaiveDate::from_ymd_opt(2026, 10, 5).unwrap()),
            0
        );
        assert_eq!(
            weekday_of(&NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()),
            6
        );
    }

    #[test]
    fn delai_minimum_respecte() {
        let now = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap().and_hms_opt(12, 0, 0).unwrap();
        let starts_at = |h: u32| now.date().and_hms_opt(h, 0, 0).unwrap();

        // Sans delai: un creneau deja commence reste trop tot, un creneau
        // futur immediat est OK.
        assert!(!respects_min_delay(starts_at(11), now, 0));
        assert!(respects_min_delay(starts_at(14), now, 0));

        // Delai de 120 minutes: 13h00 (dans 1h) refuse, 14h00 accepte.
        assert!(!respects_min_delay(starts_at(13), now, 120));
        assert!(respects_min_delay(starts_at(14), now, 120));

        // Limite exacte: debut pile a now + delai accepte.
        assert!(respects_min_delay(now + Duration::minutes(120), now, 120));
    }
}
