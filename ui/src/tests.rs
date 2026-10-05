//! Tests du module wasm. Executes dans Node.js (pas de navigateur requis):
//!
//! ```bash
//! wasm-pack test --node
//! ```
//!
//! Les tests reseau installent un stub `fetch` global qui rejoue des reponses
//! de l'API SAV: pas d'appel externe, comportement verifie de bout en bout
//! (codes HTTP, parsing, filtrage, corps des requetes). La cle API et l'URL
//! sont celles du build (build.rs lit le .env du repo).

use std::collections::BTreeMap;

use serde::Serialize as _;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test;

use serde::Serialize as _;

use crate::{SavClient, Slot, SlotsResponse};

const SLOTS_FIXTURE: &str = r#"{
  "slots_by_date": {
    "2026-10-01": [
      { "id": "202610011000", "start_at": "10:00", "booked": false },
      { "id": "202610011030", "start_at": "10:30", "booked": true },
      { "id": "202610011100", "start_at": "11:00", "booked": false }
    ],
    "2026-10-02": [
      { "id": "202610021000", "start_at": "10:00", "booked": true },
      { "id": "202610021030", "start_at": "10:30", "booked": true }
    ],
    "2026-10-03": [
      { "id": "202610031400", "start_at": "14:00", "booked": false }
    ]
  }
}"#;

const BOOKING_FIXTURE: &str = r#"{
  "slot_id": "202610031400",
  "name": "Jean Dupont",
  "email": "client@domain.com",
  "description": "Probleme de connexion",
  "start": "2026-10-03 14:00:00",
  "end": "2026-10-03 14:30:00",
  "event": { "id": 123, "code": "AB12" }
}"#;

// --- Helpers d'acces aux valeurs JS ---

fn get(obj: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(obj, &JsValue::from(key)).expect("cle JS inexistante")
}

fn get_str(obj: &JsValue, key: &str) -> String {
    get(obj, key).as_string().expect("valeur JS non string")
}

// --- Tests purs (parsing / filtrage) ---

#[wasm_bindgen_test]
fn parse_reponse_slots() {
    let response: SlotsResponse = serde_json::from_str(SLOTS_FIXTURE).unwrap();
    let days: Vec<_> = response.slots_by_date.keys().cloned().collect();
    assert_eq!(days, vec!["2026-10-01", "2026-10-02", "2026-10-03"]);
    assert_eq!(response.slots_by_date["2026-10-01"].len(), 3);
    assert_eq!(response.slots_by_date["2026-10-01"][1].id, "202610011030");
    // Les creneaux reserves restent presents avec leur statut.
    assert!(response.slots_by_date["2026-10-01"][1].booked);
    assert!(!response.slots_by_date["2026-10-01"][0].booked);
}

#[wasm_bindgen_test]
fn config_build_presente() {
    // build.rs doit avoir injecte la cle, l'URL et le titre (config.toml).
    assert!(!env!("SAV_API_KEY").is_empty());
    assert!(!env!("SAV_URL").is_empty());
    assert!(!env!("SAV_TITLE").is_empty());
}

#[wasm_bindgen_test]
fn format_date_affichage() {
    use crate::format_display_date;
    // Lundi 5 octobre 2026, premiere lettre en majuscule.
    assert_eq!(format_display_date("2026-10-05"), "Lundi 5 octobre");
    // Dimanche 11 janvier 2026.
    assert_eq!(format_display_date("2026-01-11"), "Dimanche 11 janvier");
    // Valeur non ISO passee telle quelle (robustesse).
    assert_eq!(format_display_date("pas-une-date"), "pas-une-date");
}

#[wasm_bindgen_test]
fn format_rdv_avec_heure() {
    use crate::format_booking_datetime;
    assert_eq!(format_booking_datetime("2026-10-05 14:30:00"), "Lundi 5 octobre à 14:30");
    assert_eq!(format_booking_datetime("bruit"), "bruit");
}

#[wasm_bindgen_test]
fn base_url_meme_origine_quand_vide() {
    use crate::resolve_base_url;
    // SAV_URL explicite: utilisee telle quelle.
    assert_eq!(resolve_base_url("http://localhost:8080/"), "http://localhost:8080");
    // Vide: origine de la page - absente en Node, repli localhost de dev.
    assert_eq!(resolve_base_url(""), "http://localhost:8080");
}

#[wasm_bindgen_test]
fn config_depuis_initialiseur_puis_url() {    use crate::config_value;
    let global = js_sys::global();

    // L'initialiseur (sav.js) pose __sav_config sur le global avant run_app().
    let config = js_sys::eval("({ email: 'cfg@domain.com', name: 'Jean Dupont' })").unwrap();
    js_sys::Reflect::set(&global, &JsValue::from("__sav_config"), &config).unwrap();

    assert_eq!(config_value("email").as_deref(), Some("cfg@domain.com"));
    assert_eq!(config_value("name").as_deref(), Some("Jean Dupont"));

    // Cle absente du config: repli sur le parametre d'URL (absent en Node).
    assert_eq!(config_value("inexistant"), None);

    // Valeur vide dans le config: ignoree (retour au repli).
    let config_vide = js_sys::eval("({ email: '' })").unwrap();
    js_sys::Reflect::set(&global, &JsValue::from("__sav_config"), &config_vide).unwrap();
    assert_eq!(config_value("email"), None);

    js_sys::Reflect::set(&global, &JsValue::from("__sav_config"), &JsValue::NULL).unwrap();
}

#[wasm_bindgen_test]
async fn next_booking_parse_reponse() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.__nextJson = {json};
        globalThis.fetch = (input) =>
          Promise.resolve(globalThis.__respond(globalThis.__nextJson, 200, "http://localhost:8080/api/bookings/next"));
        "#,
        helpers = JS_HELPERS,
        json = r#"{ "next_booking": { "slot_id": "202610031400", "start": "2026-10-03 14:00:00", "end": "2026-10-03 14:30:00" } }"#
    ));

    let client = client_for_tests();
    let next = client.next_booking("client@domain.com").await.expect("appel doit reussir");
    let next = next.expect("un rendez-vous doit etre trouve");
    assert_eq!(next.slot_id, "202610031400");
    assert_eq!(next.start, "2026-10-03 14:00:00");

    // Cas sans rendez-vous: next_booking absent/null.
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.fetch = (input) =>
          Promise.resolve(globalThis.__respond('{{}}', 200, "http://localhost:8080/api/bookings/next"));
        "#,
        helpers = JS_HELPERS
    ));
    let none = client.next_booking("client@domain.com").await.unwrap();
    assert!(none.is_none());
}

// --- Tests reseau via stub fetch ---

/// Installe un stub fetch global (reponses de l'API rejouees) et le retire
/// a la fin du test. Un Response Node cree manuellement n'a pas d'URL:
/// reqwest la lit (resp.url), on la redefinit donc sur l'objet. Node n'a pas
/// non plus de `window` global, requis par reqwest sur wasm.
struct FetchRestorer {
    original: JsValue,
}

impl Drop for FetchRestorer {
    fn drop(&mut self) {
        js_sys::Reflect::set(&js_sys::global(), &JsValue::from("fetch"), &self.original).unwrap();
    }
}

fn stub_fetch(responder_js: &str) -> FetchRestorer {
    let global = js_sys::global();
    let original = get(&global, "fetch");

    js_sys::Reflect::set(&global, &JsValue::from("window"), &global).unwrap();
    js_sys::eval(responder_js).expect("installation du stub fetch");

    FetchRestorer { original }
}

const JS_HELPERS: &str = r#"
globalThis.__respond = (body, status, url) => {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  const response = new Response(text, { status });
  Object.defineProperty(response, 'url', { value: url });
  return response;
};
"#;

fn client_for_tests() -> SavClient {
    SavClient::new("http://localhost:8080".to_string(), "cle-test".to_string())
}

#[wasm_bindgen_test]
async fn week_slots_via_fetch() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.__requests = [];
        globalThis.__slotsJson = {json};
        globalThis.fetch = (input, init) => {{
          const url = (input && input.url) ? input.url : String(input);
          globalThis.__requests.push({{ url }});
          return Promise.resolve(globalThis.__respond(globalThis.__slotsJson, 200, url));
        }};
        "#,
        helpers = JS_HELPERS,
        json = SLOTS_FIXTURE
    ));

    let client = client_for_tests();
    let slots = client.week_slots().await.expect("appel doit reussir");

    // Tous les creneaux sont conserves, reserves compris (affichage orange).
    assert_eq!(slots["2026-10-01"].len(), 3);
    assert_eq!(slots["2026-10-01"][0].id, "202610011000");
    assert!(!slots["2026-10-01"][0].booked);
    assert!(slots["2026-10-01"][1].booked);
    assert!(slots["2026-10-02"].iter().all(|s| s.booked));
    assert_eq!(slots["2026-10-03"][0].start_at, "14:00");

    let calls = js_sys::Array::from(&get(&js_sys::global(), "__requests"));
    assert_eq!(calls.length(), 1);
    assert_eq!(
        get_str(&calls.get(0), "url"),
        "http://localhost:8080/api/slots"
    );
}

#[wasm_bindgen_test]
async fn book_via_fetch() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.__requests = [];
        globalThis.__body = null;
        globalThis.__bookingJson = {booking};
        globalThis.fetch = (input, init) => {{
          const url = (input && input.url) ? input.url : String(input);
          globalThis.__requests.push({{ url }});
          return input.clone().text().then(body => {{
            globalThis.__body = body;
            return globalThis.__respond(globalThis.__bookingJson, 201, url);
          }});
        }};
        "#,
        helpers = JS_HELPERS,
        booking = BOOKING_FIXTURE
    ));

    let client = client_for_tests();
    let booking = client
        .book(
            "Jean Dupont".to_string(),
            "client@domain.com".to_string(),
            "Probleme de connexion".to_string(),
            "202610031400".to_string(),
        )
        .await
        .expect("reservation doit reussir");

    assert_eq!(booking.slot_id, "202610031400");
    assert_eq!(booking.start, "2026-10-03 14:00:00");

    // Le POST porte bien le slot_id et l'email dans son body JSON.
    let body = get_str(&js_sys::global(), "__body");
    let sent: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sent["slot_id"], "202610031400");
    assert_eq!(sent["name"], "Jean Dupont");
    assert_eq!(sent["email"], "client@domain.com");
    assert_eq!(sent["description"], "Probleme de connexion");

    let calls = js_sys::Array::from(&get(&js_sys::global(), "__requests"));
    assert_eq!(get_str(&calls.get(0), "url"), "http://localhost:8080/api/bookings");
}

#[wasm_bindgen_test]
async fn book_sur_slot_deja_reserve_renvoie_erreur() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.fetch = (input) =>
          Promise.resolve(globalThis.__respond(
            JSON.stringify({{ error: "slot deja reserve" }}),
            409,
            "http://localhost:8080/api/bookings"
          ));
        "#,
        helpers = JS_HELPERS
    ));

    let client = client_for_tests();
    let err = client
        .book(
            "Jean Dupont".to_string(),
            "client@domain.com".to_string(),
            "Probleme".to_string(),
            "202610031400".to_string(),
        )
        .await
        .expect_err("doit echouer avec 409");

    assert!(err.contains("409"), "erreur: {}", err);
    assert!(err.contains("slot deja reserve"), "erreur: {}", err);
}

#[wasm_bindgen_test]
async fn cle_api_manquante_renvoie_erreur() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.fetch = (input) =>
          Promise.resolve(globalThis.__respond(
            JSON.stringify({{ error: "cle d API manquante ou invalide" }}),
            401,
            "http://localhost:8080/api/slots"
          ));
        "#,
        helpers = JS_HELPERS
    ));

    let client = client_for_tests();
    let err = client.week_slots().await.expect_err("doit echouer avec 401");
    assert!(err.contains("401"), "erreur: {}", err);
}

// Le hook smoke doit rendre un objet JS simple (et non un `Map`), sinon
// Object.entries() cote script ne voit rien.
#[wasm_bindgen_test]
async fn smoke_list_rend_un_objet_js_simple() {
    let _restorer = stub_fetch(&format!(
        r#"
        {helpers}
        globalThis.__slotsJson = {json};
        globalThis.fetch = (input) =>
          Promise.resolve(globalThis.__respond(
            globalThis.__slotsJson, 200,
            "http://localhost:8080/api/slots"
          ));
        "#,
        helpers = JS_HELPERS,
        json = SLOTS_FIXTURE
    ));

    let result = crate::smoke_list_slots().await.expect("smoke doit reussir");

    let day1 = get(&result, "2026-10-01");
    let slots = js_sys::Array::from(&day1);
    assert_eq!(slots.length(), 3);
    assert_eq!(get_str(&slots.get(0), "id"), "202610011000");
}

// Garde-fou sur la forme de la map serialisee pour la page/script.
#[wasm_bindgen_test]
fn serialisation_map_en_objet_simple() {
    let mut map: BTreeMap<String, Vec<Slot>> = BTreeMap::new();
    map.insert(
        "2026-10-03".to_string(),
        vec![Slot {
            id: "202610031400".to_string(),
            start_at: "14:00".to_string(),
            booked: false,
        }],
    );
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    let value = (&map).serialize(&serializer).unwrap();
    let day = js_sys::Array::from(&get(&value, "2026-10-03"));
    assert_eq!(get_str(&day.get(0), "id"), "202610031400");
}
