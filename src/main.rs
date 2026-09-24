use actix_web::{get, web, App, HttpServer, Responder};

mod calendar;
use calendar::{get_next_week_calendar_events, CalDavConfig};

// --- API Handler ---

#[get("/api/calendar/events")]
async fn get_calendar_events() -> impl Responder {
    let config = CalDavConfig::from_env();
    let week_events = get_next_week_calendar_events(&config).await;
    web::Json(week_events)
}

// --- Entry point ---

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    println!("Demarrage du serveur...");
    println!("Endpoint: GET /api/calendar/events");

    HttpServer::new(|| {
        App::new()
            .service(get_calendar_events)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
