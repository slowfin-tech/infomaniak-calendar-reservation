//! Emails de confirmation via SMTP (lettre, STARTTLS).
//!
//! Parametres dans config.toml ([email]: sender, subject, body), secrets
//! dans le .env: SMTP_USER, SMTP_PASSWORD, SMTP_SEND_URL, SMTP_SEND_PORT.
//! L'envoi est meilleur effort: un echec SMTP n'annule pas la reservation
//! (il est trace sur stderr).

use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

const DEFAULT_SUBJECT: &str = "Votre rendez-vous Infomaniak";
const DEFAULT_BODY: &str = "\
Bonjour {name},

Votre rendez-vous est confirmé pour le {start}.

Lien visio : {link}

Motif : {description}

Cordialement,
Le service Infomaniak";

/// Donnees injectables dans les modeles d'email.
pub(crate) struct EmailData {
    pub to: String,
    pub name: String,
    pub description: String,
    pub start: String,
    pub end: String,
    /// Lien de la salle visio kMeet (vide si indisponible).
    pub link: String,
}

/// Applique les placeholders d'un modele d'email:
/// {name}, {email}, {description}, {start}, {end}, {link}.
pub(crate) fn render(template: &str, data: &EmailData) -> String {
    template
        .replace("{name}", &data.name)
        .replace("{email}", &data.to)
        .replace("{description}", &data.description)
        .replace("{start}", &data.start)
        .replace("{end}", &data.end)
        .replace("{link}", &data.link)
}

/// Echappe une valeur texte ICS (points-virgules, virgules, retours ligne).
fn ics_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// "2026-10-12 10:00:00" -> "20261012T100000" (heure locale, TZID dans DTSTART).
fn ics_datetime(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 14 {
        return digits;
    }
    format!("{}T{}", &digits[..8], &digits[8..14])
}

/// Plie une ligne ICS a 74 octets (continuation avec un espace).
fn ics_fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars() {
        if count >= 74 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += c.len_utf8();
    }
    out
}

/// Genere un fichier ICS (invitation calendar) pour le rendez-vous:
/// attache a l'email, il permet au client de l'ajouter a son agenda.
pub(crate) fn build_ics(data: &EmailData, sender: &str) -> String {
    let now_utc = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let summary = format!("Rendez-vous Infomaniak - {}", data.name);
    let mut description = data.description.clone();
    if !data.link.is_empty() {
        description.push_str(&format!("\nLien visio : {}", data.link));
    }

    let lines = [
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//slowfin-tech//infomaniak-calendar-reservation//FR".to_string(),
        "METHOD:PUBLISH".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("UID:infomaniak-{}@slowfintech.com", data.start.replace(['-', ' ', ':'], "")),
        format!("DTSTAMP:{}", now_utc),
        format!("DTSTART;TZID=Europe/Zurich:{}", ics_datetime(&data.start)),
        format!("DTEND;TZID=Europe/Zurich:{}", ics_datetime(&data.end)),
        format!("SUMMARY:{}", ics_escape(&summary)),
        format!("DESCRIPTION:{}", ics_escape(&description)),
        if data.link.is_empty() {
            String::new()
        } else {
            format!("LOCATION:{}", ics_escape(&data.link))
        },
        format!("ORGANIZER:{}", ics_escape(&format!("mailto:{}", sender))),
        format!("ATTENDEE:{}", ics_escape(&format!("mailto:{}", data.to))),
        "STATUS:CONFIRMED".to_string(),
        "END:VEVENT".to_string(),
        "END:VCALENDAR".to_string(),
    ];

    lines
        .into_iter()
        .filter(|line| !line.is_empty())
        .map(|line| ics_fold(&line))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n"
}

/// Envoie l'email de confirmation. Ne fait rien si [email] sender est vide
/// ou si le SMTP n'est pas configure cote .env.
pub(crate) async fn send_confirmation(data: &EmailData) -> Result<(), String> {
    let config = &crate::config::global().email;
    if config.sender.is_empty() {
        return Ok(());
    }

    let host = std::env::var("SMTP_SEND_URL").unwrap_or_default();
    let port: u16 = std::env::var("SMTP_SEND_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(587);
    let user = std::env::var("SMTP_USER").unwrap_or_default();
    let password = std::env::var("SMTP_PASSWORD").unwrap_or_default();

    if host.is_empty() || user.is_empty() {
        eprintln!("[email] SMTP non configure (SMTP_SEND_URL/SMTP_USER manquants), email non envoye");
        return Ok(());
    }

    let subject = render(config.subject.as_deref().unwrap_or(DEFAULT_SUBJECT), data);
    let body = render(config.body.as_deref().unwrap_or(DEFAULT_BODY), data);

    // Corps texte + invitation calendar en piece jointe ("ajouter a mon
    // agenda" cote client mail).
    let plain = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(body);
    let ics_content_type =
        ContentType::parse("text/calendar; charset=utf-8; method=PUBLISH")
            .map_err(|e| format!("content-type ics invalide: {e}"))?;
    let ics = Attachment::new("rendez-vous-infomaniak.ics".to_string())
        .body(build_ics(data, &config.sender), ics_content_type);
    let multipart = MultiPart::mixed().singlepart(plain).singlepart(ics);

    let email = Message::builder()
        .from(config.sender.parse().map_err(|e| format!("expediteur invalide: {e}"))?)
        .to(data.to.parse().map_err(|e| format!("destinataire invalide: {e}"))?)
        .subject(subject)
        .multipart(multipart)
        .map_err(|e| format!("email invalide: {e}"))?;

    // 465 = TLS implicite (relay), autres = STARTTLS (starttls_relay).
    let builder = if port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
    };
    let transport = builder
        .map_err(|e| format!("hote SMTP invalide: {e}"))?
        .port(port)
        .credentials(Credentials::new(user, password))
        .build();

    transport.send(email).await.map_err(|e| format!("envoi SMTP: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> EmailData {
        EmailData {
            to: "client@domain.com".to_string(),
            name: "Jean Dupont".to_string(),
            description: "Probleme de connexion".to_string(),
            start: "2026-10-12 10:00:00".to_string(),
            end: "2026-10-12 10:30:00".to_string(),
            link: "https://kmeet.infomaniak.com/abc123".to_string(),
        }
    }

    #[test]
    fn rend_les_placeholders() {
        let rendered = render("Bonjour {name} ({email}) - {start} -> {end} - {link} - {description}", &data());
        assert_eq!(
            rendered,
            "Bonjour Jean Dupont (client@domain.com) - 2026-10-12 10:00:00 -> 2026-10-12 10:30:00 - https://kmeet.infomaniak.com/abc123 - Probleme de connexion"
        );
    }

    #[test]
    fn placeholder_inconnu_sans_effet() {
        let rendered = render("{name} {inconnu}", &data());
        assert_eq!(rendered, "Jean Dupont {inconnu}");
    }

    #[test]
    fn corps_defaut_contient_les_infos_cles() {
        let rendered = render(DEFAULT_BODY, &data());
        assert!(rendered.contains("Jean Dupont"));
        assert!(rendered.contains("2026-10-12 10:00:00"));
        assert!(rendered.contains("https://kmeet.infomaniak.com/abc123"));
    }

    #[test]
    fn ics_genere_coherent() {
        let ics = build_ics(&data(), "noreply@slowfintech.com");
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.contains("METHOD:PUBLISH"));
        assert!(ics.contains("DTSTART;TZID=Europe/Zurich:20261012T100000"));
        assert!(ics.contains("DTEND;TZID=Europe/Zurich:20261012T103000"));
        assert!(ics.contains("SUMMARY:Rendez-vous Infomaniak - Jean Dupont"));
        assert!(ics.contains("LOCATION:https://kmeet.infomaniak.com/abc123"));
        assert!(ics.contains("ATTENDEE:mailto:client@domain.com"));
        assert!(ics.contains("STATUS:CONFIRMED"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn ics_echappe_les_valeurs() {
        // Les points-virgules et virgules sont echappes, les retours a la
        // ligne encodes en \n litteraux.
        assert_eq!(ics_escape("a,b;c\nd"), "a\\,b\\;c\\nd");
        // Date locale convertie au format ICS.
        assert_eq!(ics_datetime("2026-10-12 10:00:00"), "20261012T100000");
    }
}
