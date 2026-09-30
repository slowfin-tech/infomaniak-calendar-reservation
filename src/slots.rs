//! Module for SAV slots: 30-minute time windows derived from CalDAV events.
//!
//! A slot ID is formatted `YYYYMMDDHHMM` (start of the slot, Europe/Zurich
//! local time as returned by the calendar). Slots exist in two daily windows:
//! 10:00-12:00 and 14:00-16:00. A slot overlapping any calendar event is
//! marked booked; all slots are listed grouped by date, with their booked
//! status.

use chrono::{Duration, Local, NaiveDate, NaiveDateTime};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::calendar::{get_sliding_week_raw_events, get_sliding_week_range, CalDavConfig};

pub const SLOT_DURATION_MINUTES: i64 = 30;

const MORNING_START: u32 = 10;
const MORNING_END: u32 = 12;
const AFTERNOON_START: u32 = 14;
const AFTERNOON_END: u32 = 16;

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

/// Start times (hour, minute) of all slots in a single day.
pub(crate) fn slot_starts() -> Vec<(u32, u32)> {
    let mut starts = Vec::new();
    for (window_start, window_end) in [(MORNING_START, MORNING_END), (AFTERNOON_START, AFTERNOON_END)] {
        let mut hour = window_start;
        while hour < window_end {
            starts.push((hour, 0));
            starts.push((hour, 30));
            hour += 1;
        }
    }
    starts
}

fn slot_id(date: NaiveDate, hour: u32, minute: u32) -> String {
    format!("{}{:02}{:02}", date.format("%Y%m%d"), hour, minute)
}

/// A slot is booked if a calendar event overlaps it (half-open intervals).
pub(crate) fn is_booked(
    events: &[(String, NaiveDateTime, NaiveDateTime)],
    slot_start: NaiveDateTime,
    slot_end: NaiveDateTime,
) -> bool {
    events
        .iter()
        .any(|(_, event_start, event_end)| event_start < &slot_end && event_end > &slot_start)
}

/// Lists all slots for the sliding week grouped by date, with their booked status.
pub async fn get_week_slots(config: &CalDavConfig) -> SlotsResponse {
    let (range_start, range_end) = get_sliding_week_range();
    let events = get_sliding_week_raw_events(config).await;
    let now = Local::now().naive_local();

    let mut slots_by_date: BTreeMap<String, Vec<Slot>> = BTreeMap::new();
    let mut date = range_start;

    while date <= range_end {
        for (hour, minute) in slot_starts() {
            let slot_start = date.and_hms_opt(hour, minute, 0).unwrap();
            let slot_end = slot_start + Duration::minutes(SLOT_DURATION_MINUTES);

            // Un creneau deja passe n'est pas listable.
            if slot_end <= now {
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

    #[test]
    fn slot_starts_cover_both_windows() {
        let starts = slot_starts();
        assert_eq!(
            starts,
            vec![
                (10, 0), (10, 30), (11, 0), (11, 30),
                (14, 0), (14, 30), (15, 0), (15, 30),
            ]
        );
    }

    #[test]
    fn slot_id_format() {
        let id = slot_id(NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(), 14, 30);
        assert_eq!(id, "202609281430");
    }

    #[test]
    fn booked_on_overlap_only() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
        let slot_start = date.and_hms_opt(11, 0, 0).unwrap();
        let slot_end = slot_start + Duration::minutes(SLOT_DURATION_MINUTES);

        // Evenement a cheval sur le debut du slot -> booked
        let overlapping = vec![(
            "RDV".to_string(),
            date.and_hms_opt(10, 45, 0).unwrap(),
            date.and_hms_opt(11, 15, 0).unwrap(),
        )];
        assert!(is_booked(&overlapping, slot_start, slot_end));

        // Evenement se terminant exactement au debut du slot -> pas booked
        let adjacent = vec![(
            "RDV".to_string(),
            date.and_hms_opt(10, 30, 0).unwrap(),
            slot_start,
        )];
        assert!(!is_booked(&adjacent, slot_start, slot_end));
    }
}
