//! glory-pulse: estado de los despliegues en memoria, servido por HTTP. Solo lectura.
//!
//! Dos fuentes, elegidas con `PULSE_FUENTE`:
//! - `docker` (por defecto): los contenedores del host, por el API Docker. Es lo que corre
//!   en producción (VPS 66, Coolify).
//! - `procesos`: los proyectos del taller leídos de `/proc` (VPS 89, sin Docker), con
//!   `PULSE_RAIZ_TALLER` como raíz de los proyectos.
//!
//! Env: `PULSE_TOKEN` (obligatorio, >=32 chars; sin él no arranca), `PORT` (3000),
//! `PULSE_BIND` (`0.0.0.0`; en la 89, `127.0.0.1`), `PULSE_FUENTE`, `PULSE_RAIZ_TALLER`,
//! `HOST_ID` (o `/etc/hostname`), `DOCKER_HOST` (`http://socket-proxy:2375`),
//! `COOLIFY_API_URL` + `COOLIFY_TOKEN` (opcionales, meta lenta), `RUST_LOG`.
//!
//! Prohibido: spawn de procesos o exec; el socket Docker solo se toca vía proxy TCP.
//! registrar el token o servirlo en respuestas.

mod auth;
mod piezas;
mod procesos;
mod samplers;
mod snapshot;

use auth::{Guardia, bearer};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    middleware,
    routing::get,
};
use samplers::EstadoCompartido;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Techo de la sonda HTTP de la pieza `salud` (loopback del propio despliegue).
const TIMEOUT_SONDA_HTTP: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct App {
    estado: Arc<EstadoCompartido>,
    host_id: String,
    token_len: usize,
    /// Fuente de procesos: las piezas del detalle solo existen en ese modo.
    modo_procesos: bool,
    raiz_taller: String,
    cliente: reqwest::Client,
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
    let disco = *app.estado.disco.read().await;
    let snap = snapshot::construir(&app.host_id, lista, edad_ms, disco);
    Json(serde_json::to_value(&snap).unwrap_or(Value::Null))
}

async fn salud(State(app): State<App>) -> Json<Value> {
    let n = app.estado.contenedores.read().await.len();
    Json(json!({
        "estado": "ok",
        "schema": snapshot::SCHEMA_VERSION,
        "fuente": if app.modo_procesos { "procesos" } else { "docker" },
        "contenedores": n,
        "ts": snapshot::ahora_ms(),
    }))
}

#[derive(Deserialize)]
struct ParametrosDetalle {
    sitio: String,
}

/// [309A-2] Detalle por sitio en UNA conexión: filtra en memoria (jamás
/// toca Docker ni red). `sitio` fail-closed; sitio desconocido = lista vacía con
/// forma válida (200, no 404: el manager distingue "sin agentes" de "sitio parado").
/// En modo procesos el sitio es el nombre del proyecto del taller, no un uuid.
async fn detalle(
    State(app): State<App>,
    Query(p): Query<ParametrosDetalle>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let valido = if app.modo_procesos {
        piezas::es_sitio_valido(&p.sitio)
    } else {
        snapshot::es_uuid_valido(&p.sitio)
    };
    if !valido {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "sitio-invalido" })),
        ));
    }
    let mapa = app.estado.contenedores.read().await;
    let lista: Vec<snapshot::Contenedor> = mapa.values().map(|e| e.contenedor.clone()).collect();
    let filtrada = if app.modo_procesos {
        snapshot::filtrar_por_nombre(&lista, &p.sitio)
    } else {
        snapshot::filtrar_por_sitio(&lista, &p.sitio)
    };
    let det = snapshot::construir_detalle(&app.host_id, &p.sitio, filtrada);
    Ok(Json(serde_json::to_value(&det).unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
struct ParametrosPieza {
    sitio: String,
    nombre: String,
    lineas: Option<usize>,
}

/// [19AA-3] Piezas del detalle del taller (`salud`, `stats`, `inspeccion`, `eventos`,
/// `bd`, `diagnostico`, `logs`): los equivalentes de las que la tab vps pide a Coolify.
/// Error de dominio = 200 con `ok:false` (el tablero pinta el código); parámetros
/// inválidos = 400.
async fn pieza(
    State(app): State<App>,
    Query(p): Query<ParametrosPieza>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !app.modo_procesos {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "piezas-solo-en-procesos" })),
        ));
    }
    if !piezas::es_sitio_valido(&p.sitio) || !piezas::PIEZAS.contains(&p.nombre.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "parametros-invalidos" })),
        ));
    }
    let eventos = app.estado.eventos.read().await.clone();
    let lineas = p.lineas.unwrap_or(200);
    let (ok, datos, error) = match piezas::pieza(
        &p.nombre,
        &app.raiz_taller,
        &p.sitio,
        lineas,
        &eventos,
        &app.cliente,
    )
    .await
    {
        Ok(d) => (true, d, Value::Null),
        Err(codigo) => (false, Value::Null, json!(codigo)),
    };
    Ok(Json(json!({
        "schema": 1,
        "hostId": app.host_id,
        "ts": snapshot::ahora_ms(),
        "sitio": p.sitio,
        "pieza": p.nombre,
        "ok": ok,
        "datos": datos,
        "error": error,
    })))
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
    // [por qué] Por defecto escucha en todas las interfaces (contenedor de Coolify). En el taller
    // de la VPS 89 PULSE_BIND=127.0.0.1: un 0.0.0.0 dejaría el puerto abierto en la IP pública.
    let bind = env("PULSE_BIND", "0.0.0.0");
    let modo_procesos = env("PULSE_FUENTE", "docker") == "procesos";
    let raiz_taller = env("PULSE_RAIZ_TALLER", "/home/dev/taller");
    let estado = EstadoCompartido::nuevo();
    if modo_procesos {
        info!("fuente procesos: raíz del taller {raiz_taller}");
        tokio::spawn(procesos::bucle(raiz_taller.clone(), estado.clone()));
    } else {
        let docker_host = env("DOCKER_HOST", "http://socket-proxy:2375");
        let docker = match bollard::Docker::connect_with_http(
            &docker_host,
            120,
            bollard::API_DEFAULT_VERSION,
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("sin Docker ({docker_host}): {e}");
                std::process::exit(1);
            }
        };
        tokio::spawn(samplers::bucle_eventos(docker.clone(), estado.clone()));
        tokio::spawn(samplers::bucle_stats(docker.clone(), estado.clone()));
        let coolify_base = std::env::var("COOLIFY_API_URL").ok();
        let coolify_token = std::env::var("COOLIFY_TOKEN").ok();
        tokio::spawn(samplers::bucle_meta(
            estado.clone(),
            coolify_base,
            coolify_token,
        ));
    }
    let cliente = match reqwest::Client::builder().timeout(TIMEOUT_SONDA_HTTP).build() {
        Ok(c) => c,
        Err(e) => {
            warn!("sin cliente http para las piezas ({e}); `salud` no sondeará");
            reqwest::Client::new()
        }
    };
    let app = App {
        estado,
        host_id: host.clone(),
        token_len: guardia.longitud(),
        modo_procesos,
        raiz_taller,
        cliente,
    };
    // [por qué] El token solo aparece como longitud en logs, jamás el valor.
    info!(
        "pulse en :{puerto} host={host} fuente={} token_len={} (arranque {:?})",
        if modo_procesos { "procesos" } else { "docker" },
        app.token_len,
        Instant::now()
    );
    let rutas_protegidas = Router::new()
        .route("/snapshot", get(instantanea))
        .route("/detalle", get(detalle))
        .route("/pieza", get(pieza))
        .route_layer(middleware::from_fn_with_state(guardia.clone(), bearer))
        .with_state(app.clone());
    let app_router = Router::new()
        .route("/health", get(salud))
        .merge(rutas_protegidas)
        .with_state(app);
    let oyente = tokio::net::TcpListener::bind((bind.as_str(), puerto))
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
