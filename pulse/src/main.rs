//! glory-pulse: estado Docker en memoria, servido por HTTP. Solo lectura.
//!
//! Env: `PULSE_TOKEN` (obligatorio, 64 hex de 32B CSPRNG; sin él no arranca),
//! `PORT` (3000), `HOST_ID` (o `/etc/hostname`), `DOCKER_HOST`
//! (`http://socket-proxy:2375`), `COOLIFY_API_URL` + `COOLIFY_TOKEN`
//! (opcionales, meta lenta), `RUST_LOG`.
//!
//! Prohibido: spawn de procesos o exec; el socket Docker solo se toca vía proxy TCP.
//! registrar el token o servirlo en respuestas.

mod auth;
mod samplers;
mod snapshot;

use auth::{Guardia, bearer};
use axum::{Json, Router, extract::State, middleware, routing::get};
use samplers::EstadoCompartido;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

#[derive(Clone)]
struct App {
    estado: Arc<EstadoCompartido>,
    host_id: String,
    token_len: usize,
}

fn env(nombre: &str, defecto: &str) -> String {
    std::env::var(nombre).unwrap_or_else(|_| defecto.to_owned())
}

fn host_id() -> String {
    if let Ok(h) = std::env::var("HOST_ID")
        && !h.is_empty()
    {
        return h;
    }
    match std::fs::read_to_string("/etc/hostname") {
        Ok(h) if !h.trim().is_empty() => h.trim().to_owned(),
        _ => "vps".into(),
    }
}

/// Ruta caliente: solo memoria, ningún handler toca Docker ni red.
async fn instantanea(State(app): State<App>) -> Json<Value> {
    let mapa = app.estado.contenedores.read().await;
    let lista: Vec<snapshot::Contenedor> = mapa.values().map(|e| e.contenedor.clone()).collect();
    let edad_ms = app
        .estado
        .inicio
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    // [por qué] `edad_ms` aquí es edad del proceso (arranque); el sampler
    // refresca cada 10s, así que la frescura real la pone WM por `ts`.
    let snap = snapshot::construir(&app.host_id, lista, edad_ms);
    Json(serde_json::to_value(&snap).unwrap_or(Value::Null))
}

async fn salud(State(app): State<App>) -> Json<Value> {
    let n = app.estado.contenedores.read().await.len();
    Json(json!({
        "estado": "ok",
        "schema": snapshot::SCHEMA_VERSION,
        "contenedores": n,
        "ts": snapshot::ahora_ms(),
    }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pulse=info".into()),
        )
        .init();
    let token = std::env::var("PULSE_TOKEN").unwrap_or_default();
    if token.len() < 32 {
        eprintln!("PULSE_TOKEN ausente o corto: pulse no arranca sin secreto (>=32 chars)");
        std::process::exit(1);
    }
    let guardia = Guardia::nuevo(token);
    let host = host_id();
    let puerto: u16 = env("PORT", "3000").parse().unwrap_or(3000);
    let docker_host = env("DOCKER_HOST", "http://socket-proxy:2375");
    let docker =
        bollard::Docker::connect_with_http(&docker_host, 120, bollard::API_DEFAULT_VERSION)
            .unwrap_or_else(|e| {
                eprintln!("sin Docker ({docker_host}): {e}");
                std::process::exit(1);
            });
    let estado = EstadoCompartido::nuevo();
    tokio::spawn(samplers::bucle_eventos(docker.clone(), estado.clone()));
    tokio::spawn(samplers::bucle_stats(docker.clone(), estado.clone()));
    let coolify_base = std::env::var("COOLIFY_API_URL").ok();
    let coolify_token = std::env::var("COOLIFY_TOKEN").ok();
    tokio::spawn(samplers::bucle_meta(
        estado.clone(),
        coolify_base,
        coolify_token,
    ));
    let app = App {
        estado,
        host_id: host.clone(),
        token_len: guardia.longitud(),
    };
    // [por qué] El token solo aparece como longitud en logs, jamás el valor.
    info!(
        "pulse en :{puerto} host={host} token_len={} (arranque {:?})",
        app.token_len,
        Instant::now()
    );
    let rutas_protegidas = Router::new()
        .route("/snapshot", get(instantanea))
        .route_layer(middleware::from_fn_with_state(guardia.clone(), bearer))
        .with_state(app.clone());
    let app_router = Router::new()
        .route("/health", get(salud))
        .merge(rutas_protegidas)
        .with_state(app);
    let oyente = tokio::net::TcpListener::bind(("0.0.0.0", puerto))
        .await
        .unwrap_or_else(|e| {
            eprintln!("no escucha en {puerto}: {e}");
            std::process::exit(1);
        });
    axum::serve(oyente, app_router).await.unwrap_or_else(|e| {
        eprintln!("servidor caído: {e}");
        std::process::exit(1);
    });
}
