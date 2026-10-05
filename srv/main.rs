use actix_files::Files;
use actix_cors::Cors;
use actix_governor::{Governor, GovernorConfigBuilder};
use actix_web::{get, App, HttpRequest, HttpServer};
use serde_json::json;

mod booking;
mod calendar;
mod config;
mod slots;
use calendar::{get_sliding_week_calendar_events, CalDavConfig};

/// Verifie la cle d'API (Authorization: Bearer <SAV_API_KEY>) sur chaque
/// endpoint. Retourne une reponse 401 si l'appelant n'est pas autorise.
pub(crate) fn check_api_key(req: &HttpRequest) -> Result<(), actix_web::HttpResponse> {
    let api_key = std::env::var("SAV_API_KEY").expect("SAV_API_KEY doit etre defini");

    let authorized = req
        .headers()
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {}", api_key));

    if authorized {
        Ok(())
    } else {
        Err(actix_web::HttpResponse::Unauthorized()
            .json(json!({"error": "cle d API manquante ou invalide"})))
    }
}

#[get("/api/calendar/events")]
async fn get_calendar_events(req: HttpRequest) -> actix_web::HttpResponse {
    if let Err(resp) = check_api_key(&req) {
        return resp;
    }
    let config = CalDavConfig::from_env();
    let response = get_sliding_week_calendar_events(&config).await;
    actix_web::HttpResponse::Ok().json(response)
}

#[get("/api/slots")]
async fn get_slots(req: HttpRequest) -> actix_web::HttpResponse {
    if let Err(resp) = check_api_key(&req) {
        return resp;
    }
    let caldav_config = CalDavConfig::from_env();
    let slots_config = slots::SlotsConfig::from_app_config(crate::config::global());
    let response = slots::get_week_slots(&caldav_config, &slots_config).await;
    actix_web::HttpResponse::Ok().json(response)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    println!("Demarrage du serveur...");
    println!("Endpoint: GET /api/calendar/events");
    println!("Endpoint: GET /api/slots");
    println!("Endpoint: POST /api/bookings");

    // Rate limiting par IP en garde-fou contre le spam de reservations :
    // 1 requete reallouee toutes les 10 secondes, burst de 50.
    let governor_conf = GovernorConfigBuilder::default()
        .seconds_per_request(10)
        .burst_size(50)
        .finish()
        .unwrap();

    // CORS ouvert : le composant UI (projet separe) appelle cette API depuis
    // une autre origine; l'acces reste gate par la cle d'API Bearer.
    // Configuration non-secret depuis config.toml, chargee une seule fois.
    let port = config::global().server.port;

    HttpServer::new(move || {
        App::new()
            .wrap(Governor::new(&governor_conf))
            .wrap(Cors::permissive())
            .service(get_calendar_events)
            .service(get_slots)
            .service(booking::create_booking)
            .service(booking::get_next_booking)
            .service(booking::cancel_booking)
            // Bundle UI (sav.js, sav.css, pkg/, page de demo) servi par le
            // meme hote - enregistre en dernier, ne capture que /api/ ne
            // touche pas.
            .service(Files::new("/", "./ui/static").index_file("index.html"))
    })
    .bind(format!("0.0.0.0:{}", port))?
    .run()
    .await
}
