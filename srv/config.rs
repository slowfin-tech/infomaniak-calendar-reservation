//! Configuration non-secret, chargee depuis `config.toml` (chemin par
//! defaut, surchargeable via `CONFIG_PATH`). Les secrets restent dans le
//! `.env` (CALDAV_LOGIN, CALDAV_PASSWORD, KMEET_API_TOKEN, SAV_API_KEY).
//!
//! Chargee une seule fois au demarrage puis accesible via `config::global()`.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

// --- Modeles ---

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub ui: UiConfig,
    pub caldav: CalDavSettings,
    pub infomaniak: InfomaniakSettings,
    pub slots: SlotsConfigToml,
    pub booking: BookingConfig,
    pub email: EmailConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Port d'ecoute (defaut 8080).
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig { port: 8080 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Titre affiche par l'interface.
    pub title: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { title: "SAV - Rendez-vous".to_string() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CalDavSettings {
    /// URL de base du serveur CalDAV.
    pub url: String,
    /// Chemin du calendar.
    pub calendar_uri: String,
}

impl Default for CalDavSettings {
    fn default() -> Self {
        CalDavSettings {
            url: "https://sync.infomaniak.com/".to_string(),
            calendar_uri: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InfomaniakSettings {
    /// ID du calendar Infomaniak où creer les evenements.
    pub calendar_id: u64,
    /// Hostname kMeet pour les liens visio.
    pub visio_base_url: String,
}

impl Default for InfomaniakSettings {
    fn default() -> Self {
        InfomaniakSettings { calendar_id: 0, visio_base_url: String::new() }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SlotsConfigToml {
    /// Duree des creneaux en minutes (defaut 30 si absent).
    pub duration_minutes: Option<i64>,
    /// Delai minimum avant reservation en minutes (defaut 0 si absent).
    pub min_delay_minutes: Option<i64>,
    /// Periodes par jour; None = valeurs par defaut, Some = remplacement
    /// complet (jours non listes = fermes).
    pub periods: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BookingConfig {
    /// Modele de la description envoyee a l'API kMeet lors de la creation
    /// de la reservation. Placeholders: {description}, {email}, {start}, {end}.
    /// Defaut: "{description}\n\nContact: {email}".
    pub description_template: Option<String>,
}

impl Default for BookingConfig {
    fn default() -> Self {
        BookingConfig { description_template: None }
    }
}

/// Section [email]: envoi d'un email de confirmation apres une reservation.
/// `sender` vide (section absente) = pas d'email. Les identifiants SMTP
/// (secrets) restent dans le .env: SMTP_USER, SMTP_PASSWORD,
/// SMTP_SEND_URL, SMTP_SEND_PORT.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EmailConfig {
    /// Adresse de l'expediteur (ex: "sav@slowfintech.com").
    pub sender: String,
    /// Modele d'objet; placeholders: {name}, {email}, {description},
    /// {start}, {end}, {link}.
    pub subject: Option<String>,
    /// Modele de corps (texte brut); memes placeholders.
    pub body: Option<String>,
}

impl Default for EmailConfig {
    fn default() -> Self {
        EmailConfig { sender: String::new(), subject: None, body: None }
    }
}

// --- Chargement ---

impl AppConfig {
    /// Lit config.toml (ou CONFIG_PATH). Fichier absent: valeurs par defaut.
    /// Fichier present mais invalide: erreur explicite (echec au demarrage).
    pub fn load() -> Self {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
        let path = PathBuf::from(path);

        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                panic!("config.toml ({}) invalide: {}", path.display(), e)
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("config.toml introuvable ({}) : valeurs par defaut", path.display());
                AppConfig::default()
            }
            Err(e) => panic!("config.toml ({}) illisible: {}", path.display(), e),
        }
    }
}

static CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Configuration globale, chargee au premier acces.
pub fn global() -> &'static AppConfig {
    CONFIG.get_or_init(AppConfig::load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_complete() {
        let config: AppConfig = toml::from_str(
            r#"
            [server]
            port = 9090

            [ui]
            title = "Mon SAV"

            [caldav]
            url = "https://caldav.example.com/"
            calendar_uri = "/calendars/u/abc/"

            [infomaniak]
            calendar_id = 42
            visio_base_url = "kmeet.example.com"

            [slots]
            duration_minutes = 45
            min_delay_minutes = 120

            [slots.periods]
            mon = ["09:00-12:00"]
            sat = []
            "#,
        )
        .unwrap();

        assert_eq!(config.server.port, 9090);
        assert_eq!(config.ui.title, "Mon SAV");
        assert_eq!(config.caldav.url, "https://caldav.example.com/");
        assert_eq!(config.infomaniak.calendar_id, 42);
        assert_eq!(config.slots.duration_minutes, Some(45));
        assert_eq!(config.slots.min_delay_minutes, Some(120));
        let periods = config.slots.periods.unwrap();
        assert_eq!(periods["mon"], vec!["09:00-12:00"]);
        assert!(periods["sat"].is_empty());
        assert!(!periods.contains_key("sun"));
    }

    #[test]
    fn defauts_quand_absent() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.ui.title, "SAV - Rendez-vous");
        assert_eq!(config.slots.duration_minutes, None);
        assert!(config.slots.periods.is_none());
    }
}
