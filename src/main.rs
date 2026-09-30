use actix_governor::{Governor, GovernorConfigBuilder};
use actix_web::{get, App, HttpRequest, HttpServer};
use serde_json::json;

mod booking;
mod calendar;
mod slots;
use calendar::{get_sliding_week_calendar_events, CalDavConfig};
use slots::get_week_slots;

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
    let config = CalDavConfig::from_env();
    let response = get_week_slots(&config).await;
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

    HttpServer::new(move || {
        App::new()
            .wrap(Governor::new(&governor_conf))
            .service(get_calendar_events)
            .service(get_slots)
            .service(booking::create_booking)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
