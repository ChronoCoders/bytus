use bytus::config::Config;
use bytus::db;
use bytus::routes;
use bytus::state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = Config::from_env().unwrap_or_else(|e| {
        eprintln!("configuration error: {e}");
        std::process::exit(1);
    });

    tracing::info!(fee_bps = cfg.fee_bps, byts_rate = cfg.byts_rate, "configuration loaded");

    let pool = db::connect(&cfg.database_url).await.unwrap_or_else(|e| {
        eprintln!("database connection error: {e}");
        std::process::exit(1);
    });

    let app = routes::app(AppState {
        pool,
        jwt_secret: cfg.jwt_secret,
        fee_bps: cfg.fee_bps,
        byts_rate: cfg.byts_rate,
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .unwrap_or_else(|e| {
            eprintln!("failed to bind: {e}");
            std::process::exit(1);
        });

    tracing::info!(
        "listening on {}",
        listener
            .local_addr()
            .unwrap_or_else(|_| "unknown".parse().unwrap())
    );

    axum::serve(listener, app).await.unwrap_or_else(|e| {
        eprintln!("server error: {e}");
        std::process::exit(1);
    });
}
