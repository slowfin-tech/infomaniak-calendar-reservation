//! Interface de réservation Infomaniak compilée en wasm: client API + rendu Yew.
//!
//! Aucune configuration n'est compilee dans le module: la cle d'API et les
//! textes sont charges au demarrage via GET /api/config.
//! L'email du client est passe en parametre d'URL de la page: ?email=...
//!
//! La page hote (static/) ne charge PAS le wasm a l'ouverture: infomaniak.js
//! importe dynamiquement /pkg/infomaniak_ui.js au clic puis appelle run_app().

use std::collections::BTreeMap;

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use yew::prelude::*;

// --- Modeles miroirs de l'API ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slot {
    pub id: String,
    pub start_at: String,
    pub booked: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct SlotsResponse {
    slots_by_date: BTreeMap<String, Vec<Slot>>,
    /// Duree d'un creneau en minutes (defaut 0 = inconnue: pas de
    /// decoupage par pauses).
    #[serde(default)]
    duration_minutes: i64,
}

#[derive(Serialize)]
struct BookingRequest {
    name: String,
    email: String,
    description: String,
    slot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BookingResponse {
    slot_id: String,
    name: String,
    email: String,
    description: String,
    start: String,
    end: String,
    event: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextBooking {
    pub slot_id: String,
    pub start: String,
    pub end: String,
    /// Lien de la salle visio (absent si l'evenement n'en porte pas).
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NextBookingResponse {
    #[serde(default)]
    next_booking: Option<NextBooking>,
}

// --- Configuration runtime ---

/// Configuration publique servie par GET /api/config. `ui` est la section
/// [ui] de config.toml servie telle quelle (title, placeholder, ...).
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
struct RuntimeConfig {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    ui: std::collections::BTreeMap<String, String>,
}

/// Texte d'interface depuis la config runtime, avec repli par defaut.
fn ui_text(ui: &std::collections::BTreeMap<String, String>, key: &str, fallback: &str) -> String {
    ui.get(key).cloned().unwrap_or_else(|| fallback.to_string())
}

/// Resout la base de l'API: vide = origine de la page (deploiement
/// mono-origine, ex: conteneur servant UI + API). Hors navigateur (Node),
/// retour au localhost de dev.
fn resolve_base_url(base: &str) -> String {
    if !base.is_empty() {
        return base.trim_end_matches('/').to_string();
    }
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .filter(|origin| !origin.is_empty())
        .unwrap_or_else(|| "http://localhost:8080".to_string())
}

/// Recupere la configuration runtime depuis GET /api/config (endpoint non
/// authentifie: le module n'a pas encore la cle).
async fn fetch_runtime_config(base: &str) -> Result<RuntimeConfig, String> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/config", base))
        .send()
        .await
        .map_err(|e| format!("erreur reseau: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("erreur API: {} {}", status, body));
    }

    response.json().await.map_err(|e| format!("reponse illisible: {e}"))
}

#[derive(Clone)]
pub(crate) struct ApiClient {
    base_url: String,
    api_key: String,
}

impl ApiClient {
    fn new(base_url: String, api_key: String) -> ApiClient {
        ApiClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// Client depuis la configuration runtime (fetch de /api/config).
    pub(crate) async fn from_runtime_config() -> Result<ApiClient, String> {
        let base = resolve_base_url(&config_value("apiUrl").unwrap_or_default());
        let config = fetch_runtime_config(&base).await?;
        Ok(ApiClient::new(base, config.api_key))
    }

    /// Recupere la configuration runtime (api/config ne demande pas la cle:
    /// le module ne l'a pas encore).
    pub(crate) async fn fetch_config() -> Result<RuntimeConfig, String> {
        let base = resolve_base_url(&config_value("apiUrl").unwrap_or_default());
        fetch_runtime_config(&base).await
    }

    /// Retourne tous les creneaux de la semaine, groupes par date, avec leur
    /// statut `booked` (l'affichage distingue libres / reserves).
    pub(crate) async fn week_slots(&self) -> Result<SlotsResponse, String> {
        let response = self.get("/api/slots").send().await.map_err(|e| format!("erreur reseau: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("erreur API: {} {}", status, body));
        }

        let slots: SlotsResponse = response.json().await.map_err(|e| format!("reponse illisible: {e}"))?;
        Ok(slots)
    }

    /// Reserve un slot pour le compte du client donne.
    pub(crate) async fn book(
        &self,
        name: String,
        email: String,
        description: String,
        slot_id: String,
    ) -> Result<BookingResponse, String> {
        let body = BookingRequest {
            name,
            email,
            description,
            slot_id,
        };

        let response = self
            .post("/api/bookings")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("erreur reseau: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("reservation impossible: {} {}", status, body));
        }

        response.json().await.map_err(|e| format!("reponse illisible: {e}"))
    }

    /// Prochain rendez-vous de cet utilisateur (null s'il n'en a pas).
    pub(crate) async fn next_booking(&self, email: &str) -> Result<Option<NextBooking>, String> {
        let response = reqwest::Client::new()
            .get(format!("{}/api/bookings/next", self.base_url))
            .query(&[("email", email)])
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| format!("erreur reseau: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("erreur API: {} {}", status, body));
        }

        let parsed: NextBookingResponse = response
            .json()
            .await
            .map_err(|e| format!("reponse illisible: {e}"))?;
        Ok(parsed.next_booking)
    }

    /// Annule le prochain rendez-vous de cet utilisateur.
    pub(crate) async fn cancel_booking(&self, email: &str) -> Result<(), String> {
        let response = self
            .post("/api/bookings/cancel")
            .json(&serde_json::json!({ "email": email }))
            .send()
            .await
            .map_err(|e| format!("erreur reseau: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("annulation impossible: {} {}", status, body));
        }
        Ok(())
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .get(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.api_key)
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.api_key)
    }
}

// --- Lecture des parametres de la page ---

/// Recupere un parametre d'URL de la page (ex: ?email=client@domain.com).
fn query_param(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name)
}

/// Valeur de configuration pour l'application: d'abord celle de
/// l'initialiseur (window.__infomaniak_config, posee par infomaniak.js), puis le
/// parametre d'URL en repli.
fn config_value(key: &str) -> Option<String> {
    let global = js_sys::global();
    if let Ok(config) = js_sys::Reflect::get(&global, &JsValue::from("__infomaniak_config")) {
        if !config.is_undefined() && !config.is_null() {
            let value = js_sys::Reflect::get(&config, &JsValue::from(key)).ok();
            if let Some(value) = value.and_then(|v| v.as_string()) {
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    query_param(key)
}

// --- Affichage des dates ---

const WEEKDAYS_FR: [&str; 7] =
    ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// "2026-10-05" -> "Lundi 5 octobre" (clé ISO telle que renvoyee par l'API).
fn format_display_date(iso: &str) -> String {    match chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
        Ok(date) => {
            let weekday = WEEKDAYS_FR[date.weekday().num_days_from_monday() as usize];
            // Premiere lettre en majuscule.
            let mut chars = weekday.chars();
            let weekday = match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            };
            format!("{} {} {}", weekday, date.day(), MONTHS_FR[date.month0() as usize])
        }
        Err(_) => iso.to_string(),
    }
}

/// "2026-10-05 14:30:00" -> "Lundi 5 octobre à 14:30".
fn format_booking_datetime(raw: &str) -> String {
    match chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        Ok(datetime) => {
            let date = datetime.date().format("%Y-%m-%d").to_string();
            format!("{} à {}", format_display_date(&date), datetime.format("%H:%M"))
        }
        Err(_) => raw.to_string(),
    }
}

// --- Decoupage des creneaux par pauses ---

/// "14:30" -> 870 minutes depuis minuit.
fn hm_to_minutes(hm: &str) -> Option<i64> {
    let (hours, minutes) = hm.trim().split_once(':')?;
    Some(hours.trim().parse::<i64>().ok()? * 60 + minutes.trim().parse::<i64>().ok()?)
}

/// Decoupe les creneaux d'une journee en blocs continus, separes par les
/// pauses (trou superieur a la duree d'un creneau, ex: la pause du midi).
pub(crate) fn slot_blocks(slots: &[Slot], duration_minutes: i64) -> Vec<Vec<Slot>> {
    let mut blocks: Vec<Vec<Slot>> = Vec::new();
    let mut prev_end: Option<i64> = None;

    for slot in slots {
        let start = hm_to_minutes(&slot.start_at);
        // Trou apres le creneau precedent: nouvelle periode.
        let is_break = matches!((start, prev_end), (Some(start), Some(prev_end))
            if duration_minutes > 0 && prev_end < start);

        match (blocks.last_mut(), is_break) {
            (Some(_), true) => blocks.push(vec![slot.clone()]),
            (Some(block), false) => block.push(slot.clone()),
            (None, _) => blocks.push(vec![slot.clone()]),
        }

        prev_end = start.map(|start| {
            if duration_minutes > 0 {
                start + duration_minutes
            } else {
                start
            }
        });
    }

    blocks
}

// --- Composant Yew ---

#[function_component]
fn App() -> Html {
    let email = use_memo((), |_| config_value("email"));
    let name = use_memo((), |_| config_value("name").or_else(|| config_value("email")));
    let slots = use_state(|| SlotsResponse {
        slots_by_date: BTreeMap::new(),
        duration_minutes: 0,
    });
    let status = use_state(String::new);
    let is_error = use_state(|| false);
    // Creneau selectionne: (date, slot). Bascule la vue vers le formulaire.
    let selected = use_state(|| Option::<(String, Slot)>::None);
    let description = use_state(String::new);
    // Prochain rendez-vous de l'utilisateur (affichage dedie).
    let next_booking = use_state(|| Option::<NextBooking>::None);

    // Ouverture de la modal: montee au premier clic du bouton hote, puis
    // re-ouverte via window.__infomaniakToggle pose par l'effet de montage.
    let open = use_state(|| true);

    // Configuration runtime: chargee au demarrage depuis GET /api/config
    // (plus rien n'est cuit au build). Client API derive de cette config.
    let runtime_config = use_state(|| None::<RuntimeConfig>);
    let client = use_memo((*runtime_config).clone(), move |config| {
        config.clone().map(|config| {
            ApiClient::new(
                resolve_base_url(&config_value("apiUrl").unwrap_or_default()),
                config.api_key.clone(),
            )
        })
    });

    // Rafraichit la liste des creneaux et le prochain rendez-vous.
    let refresh = {
        let slots = slots.clone();
        let status = status.clone();
        let is_error = is_error.clone();
        let next_booking = next_booking.clone();
        let client = client.clone();
        let email = email.clone();
        Callback::from(move |_: ()| {
            let Some(client) = (*client).clone() else {
                return;
            };
            let slots = slots.clone();
            let status = status.clone();
            let is_error = is_error.clone();
            let next_booking = next_booking.clone();
            let email = (*email).clone();
            status.set("Chargement des creneaux...".into());
            is_error.set(false);
            wasm_bindgen_futures::spawn_local(async move {
                // Trace du rendez-vous de l'utilisateur d'abord: s'il en a un,
                // le calendrier ne sera pas affiche du tout.
                if let Some(email) = email.clone() {
                    match client.next_booking(&email).await {
                        Ok(next) => next_booking.set(next),
                        Err(e) => {
                            is_error.set(true);
                            status.set(e);
                        }
                    }
                }
                match client.week_slots().await {
                    Ok(map) => {
                        slots.set(map);
                        status.set(String::new());
                    }
                    Err(e) => {
                        is_error.set(true);
                        status.set(e);
                    }
                }
            });
        })
    };

    // Charge la configuration au montage (le module n'existe qu'apres le
    // clic de la page hote). Les creneaux suivent une fois le client pret.
    {
        let runtime_config = runtime_config.clone();
        let status = status.clone();
        let is_error = is_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match ApiClient::fetch_config().await {
                    Ok(config) => runtime_config.set(Some(config)),
                    Err(e) => {
                        is_error.set(true);
                        status.set(e);
                    }
                }
            });
        });
    }

    // Rafraichit les donnees quand le client (issu de la config) est pret.
    {
        let refresh = refresh.clone();
        use_effect_with(runtime_config.clone(), move |config| {
            if config.is_some() {
                refresh.emit(());
            }
        });
    }

    // Expose au chargeur (infomaniak.js) la reouverture de la modal: le bouton hote
    // appelle window.__infomaniakToggle() aux clics suivant le premier chargement.
    {
        let open = open.clone();
        use_effect_with((), move |_| {
            let toggle = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
                open.set(true);
            });
            js_sys::Reflect::set(
                &js_sys::global(),
                &JsValue::from("__infomaniakToggle"),
                toggle.as_ref(),
            )
            .expect("pose de __infomaniakToggle");
            std::mem::forget(toggle);
        });
    }

    // Fermeture de la modal (clic sur l'overlay ou sur le bouton de fermeture).
    let close_modal = {
        let open = open.clone();
        Callback::from(move |(): ()| open.set(false))
    };
    // Empêche le clic dans la modal de se propager à l'overlay.
    let swallow_click = Callback::from(|e: MouseEvent| e.stop_propagation());

    // Reserve le slot selectionne avec la description saisie, puis
    // rafraichit la liste.
    let confirm_booking = {
        let selected = selected.clone();
        let description = description.clone();
        let status = status.clone();
        let is_error = is_error.clone();
        let refresh = refresh.clone();
        let client = client.clone();
        let email = email.clone();
        let name = name.clone();
        Callback::from(move |(): ()| {
            let Some((date, slot)) = (*selected).clone() else {
                return;
            };
            let Some(email) = (*email).clone() else {
                is_error.set(true);
                status.set("Email manquant: chargez la page avec ?email=...".into());
                return;
            };
            let name = (*name).clone().unwrap_or_else(|| email.clone());
            let description_text = (*description).trim().to_string();
            if description_text.is_empty() {
                is_error.set(true);
                status.set("Decrivez le probleme avant de confirmer.".into());
                return;
            }

            let Some(client) = (*client).clone() else {
                return;
            };
            let status = status.clone();
            let is_error = is_error.clone();
            let refresh = refresh.clone();
            let selected = selected.clone();
            let description_state = description.clone();
            status.set("Reservation en cours...".into());
            is_error.set(false);
            wasm_bindgen_futures::spawn_local(async move {
                match client.book(name, email, description_text, slot.id).await {
                    Ok(booking) => {
                        status.set(format!(
                            "Reserve le {} : {} -> {}",
                            format_display_date(&date),
                            booking.start,
                            booking.end
                        ));
                        // Quitte le formulaire: le rafraichissement affichera
                        // la vue "prochain rendez-vous".
                        selected.set(None);
                        description_state.set(String::new());
                        refresh.emit(());
                    }
                    Err(e) => {
                        is_error.set(true);
                        status.set(e);
                    }
                }
            });
        })
    };

    let cancel_selection = {
        let selected = selected.clone();
        let description = description.clone();
        let status = status.clone();
        let is_error = is_error.clone();
        Callback::from(move |(): ()| {
            selected.set(None);
            description.set(String::new());
            status.set(String::new());
            is_error.set(false);
        })
    };

    // Annule le rendez-vous de l'utilisateur puis rafraichit l'affichage.
    let cancel_booking = {
        let status = status.clone();
        let is_error = is_error.clone();
        let refresh = refresh.clone();
        let client = client.clone();
        let email = email.clone();
        Callback::from(move |(): ()| {
            let Some(client) = (*client).clone() else {
                return;
            };
            let Some(email) = (*email).clone() else {
                return;
            };
            let status = status.clone();
            let is_error = is_error.clone();
            let refresh = refresh.clone();
            status.set("Annulation...".into());
            is_error.set(false);
            wasm_bindgen_futures::spawn_local(async move {
                match client.cancel_booking(&email).await {
                    Ok(()) => {
                        status.set("Rendez-vous annulé.".into());
                        refresh.emit(());
                    }
                    Err(e) => {
                        is_error.set(true);
                        status.set(e);
                    }
                }
            });
        })
    };

    // Saisie de la description dans le formulaire.
    let on_description_input = {
        let description = description.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(textarea) = e.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                description.set(textarea.value());
            }
        })
    };

    let status_class = if *is_error { "error" } else { "" };

    // Textes de l'interface depuis la configuration runtime (replis par
    // defaut si absents de [ui]).
    let cfg = (*runtime_config).clone();
    let ui_texts = cfg.as_ref().map(|config| config.ui.clone()).unwrap_or_default();
    let text = move |key: &str, fallback: &str| ui_text(&ui_texts, key, fallback);

    // Vue formulaire: un creneau est selectionne, on demande la description.
    let content = if cfg.is_none() {
        html! { <p>{ "Chargement de la configuration..." }</p> }
    } else if let Some((date, slot)) = &*selected {
        html! {
            <>
                <h2>{ format!("Créneau du {} à {}", format_display_date(&date), slot.start_at) }</h2>
                if let Some(email) = &*email {
                    <p>{ format!("Rendez-vous pour {} ({})", (*name).clone().unwrap_or_else(|| email.clone()), email) }</p>
                } else {
                    <p class="error">
                        { "Email manquant: chargez la page avec ?email=client@domain.com" }
                    </p>
                }
                <p class={status_class}>{ (*status).clone() }</p>
                <div class="booking-form">
                    <label for="description">
                        { text("description_label", "Décrivez le problème :") }
                    </label>
                    <textarea
                        id="description"
                        rows="4"
                        placeholder={ text("placeholder", "Problème de connexion au boîtier...") }
                        value={(*description).clone()}
                        oninput={on_description_input}
                    />
                    <div class="actions">
                        <button class="primary" onclick={confirm_booking.reform(|_| ())}>
                            { "Confirmer la réservation" }
                        </button>
                        <button onclick={cancel_selection.reform(|_| ())}>{ "Annuler" }</button>
                    </div>
                </div>
            </>
        }
    }
    // Vue rendez-vous: un utilisateur ayant deja un rendez-vous ne voit pas
    // le calendrier.
    else if let Some(booking) = &*next_booking {
        let on_cancel = {
            let cancel_booking = cancel_booking.clone();
            let runtime_config = runtime_config.clone();
            Callback::from(move |_| {
                // Confirmation avant l'annulation effective (message
                // configurable via [ui] cancel_confirm).
                let message = (*runtime_config)
                    .as_ref()
                    .map(|config| ui_text(&config.ui, "cancel_confirm", "Annuler votre rendez-vous ?"))
                    .unwrap_or_else(|| "Annuler votre rendez-vous ?".to_string());
                let confirmed = web_sys::window()
                    .map(|window| window.confirm_with_message(&message).unwrap_or(false))
                    .unwrap_or(true);
                if confirmed {
                    cancel_booking.emit(());
                }
            })
        };
        html! {
            <>
                <h2>{ "Votre rendez-vous" }</h2>
                <div class="next-booking">
                    { format!("Votre prochain rendez-vous : {}", format_booking_datetime(&booking.start)) }
                </div>
                <p>{ "Un seul rendez-vous à la fois : il n'est pas possible d'en réserver un autre." }</p>
                <p class={status_class}>{ (*status).clone() }</p>
                // Lien vers la salle visio, quand l'evenement le porte.
                if let Some(link) = &booking.link {
                    <a class="visio-link" href={link.clone()} target="_blank" rel="noopener">
                        // Picto camera video
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="16"
                            height="16"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <path d="m23 7-7 5 7 5V7z" />
                            <rect x="1" y="5" width="15" height="14" rx="2" ry="2" />
                        </svg>
                        { "Rejoindre la visio" }
                    </a>
                }
                <div class="actions appointment-actions">
                    <button class="danger" onclick={on_cancel}>{ "Annuler le rendez-vous" }</button>
                </div>
            </>
        }
    }
    // Vue liste: les creneaux sont cliquables.
    else {
        html! {
            <>
                if let Some(email) = &*email {
                    <h2>{ format!("Créneaux pour {}", (*name).clone().unwrap_or_else(|| email.clone())) }</h2>
                    <p class="hint">{ format!("Compte : {email}") }</p>
                } else {
                    <p class="error">
                        { "Email manquant: chargez la page avec ?email=client@domain.com" }
                    </p>
                }
                <p class={status_class}>{ (*status).clone() }</p>
                {
                    for (*slots).slots_by_date.iter().filter(|(_, list)| !list.is_empty()).map(|(date, list)| {
                        let blocks = slot_blocks(list, (*slots).duration_minutes);
                        let block_count = blocks.len();
                        html! {
                            <div class="day">
                                <h3>{ format_display_date(date) }</h3>
                                <div class="slots">
                                    { for blocks.into_iter().enumerate().map(|(index, block)| {
                                        html! {
                                            <>
                                                { for block.iter().map(|slot| {
                                                    if slot.booked {
                                                        // Deja reserve: affiche, non cliquable.
                                                        html! {
                                                            <span class="slot booked">
                                                                { slot.start_at.clone() }
                                                            </span>
                                                        }
                                                    } else {
                                                        // Clic sur le creneau: bascule vers le formulaire.
                                                        let onclick = {
                                                            let selected = selected.clone();
                                                            let description = description.clone();
                                                            let status = status.clone();
                                                            let is_error = is_error.clone();
                                                            let date = date.clone();
                                                            let slot = slot.clone();
                                                            Callback::from(move |_| {
                                                                selected.set(Some((date.clone(), slot.clone())));
                                                                description.set(String::new());
                                                                status.set(String::new());
                                                                is_error.set(false);
                                                            })
                                                        };
                                                        html! {
                                                            <span class="slot clickable" {onclick}>
                                                                { slot.start_at.clone() }
                                                            </span>
                                                        }
                                                    }
                                                })}
                                                // Marqueur de pause entre deux periodes:
                                                // carre gris en ligne (ecran large), trait
                                                // + retour a la ligne en mobile (voir CSS).
                                                if index + 1 < block_count {
                                                    <span class="break" />
                                                }
                                            </>
                                        }
                                    })}
                                </div>
                            </div>
                        }
                    })
                }
            </>
        }
    };

    // Modal fermee: rien a afficher (le bouton hote la reouvrira).
    if !*open {
        return html! {};
    }

    html! {
        <div class="infomaniak-overlay" onclick={close_modal.reform(|_| ())}>
            <div class="infomaniak-modal" role="dialog" aria-modal="true" onclick={swallow_click}>
                <div class="infomaniak-modal-header">
                    <h1>{ (*runtime_config).as_ref().map(|config| ui_text(&config.ui, "title", "Infomaniak - Rendez-vous")).unwrap_or_else(|| "Infomaniak - Rendez-vous".to_string()) }</h1>
                    <button class="infomaniak-close" aria-label="Fermer" onclick={close_modal.reform(|_| ())}>
                        { "×" }
                    </button>
                </div>
                { content }
            </div>
        </div>
    }
}

#[wasm_bindgen]
pub fn run_app() {
    console_error_panic_hook::set_once();
    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("infomaniak-root"));
    match root {
        Some(root) => yew::Renderer::<App>::with_root(root).render(),
        None => yew::Renderer::<App>::new().render(),
    };
}

// --- Hooks de test manuel (scripts/smoke.mjs) ---

#[wasm_bindgen]
pub async fn smoke_list_slots() -> Result<JsValue, JsValue> {
    let slots = ApiClient::from_runtime_config()
        .await
        .map_err(|e| JsValue::from_str(&e))?
        .week_slots()
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    // Objet JS simple (et non `Map`) pour Object.entries() cote script.
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    (&slots.slots_by_date)
        .serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub async fn smoke_book(
    name: String,
    email: String,
    description: String,
    slot_id: String,
) -> Result<JsValue, JsValue> {
    let booking = ApiClient::from_runtime_config()
        .await
        .map_err(|e| JsValue::from_str(&e))?
        .book(name, email, description, slot_id)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    serde_wasm_bindgen::to_value(&booking).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests;
