//! Tipos del snapshot: contrato público v1 (`schema = 1`).
//!
//! [por qué] El esquema vive también en `schema/snapshot.schema.json`; este
//! módulo es su espejo en Rust. Cambios incompatibles = `schema` nueva +
//! entrada en CHANGELOG (el test `tests/contrato.rs` lo vigila).

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Versión del contrato. WM rechaza cualquier otra (fail-closed).
pub const SCHEMA_VERSION: u32 = 1;
/// Techo v1 del modelo de escala: recorte determinista (por nombre) como red.
pub const MAX_CONTENEDORES: usize = 50;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recursos {
    pub cpu_pct: f64,
    pub mem_usada: u64,
    pub mem_limite: u64,
    pub blk_ro: u64,
    pub blk_wo: u64,
    pub net_rx: u64,
    pub net_tx: u64,
}

impl Default for Recursos {
    fn default() -> Self {
        Self {
            cpu_pct: 0.0,
            mem_usada: 0,
            mem_limite: 0,
            blk_ro: 0,
            blk_wo: 0,
            net_rx: 0,
            net_tx: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstadoContenedor {
    pub estado: String,
    pub salud: String,
    pub reinicios: i64,
    pub desde: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contenedor {
    pub id12: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub nombre: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sitio_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dominio: Option<String>,
    pub estado: EstadoContenedor,
    pub recursos: Recursos,
    #[serde(default)]
    pub puertos: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frescura {
    pub fuente: &'static str,
    pub edad_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema: u32,
    pub host_id: String,
    pub ts: u64,
    pub contenedores: Vec<Contenedor>,
    pub frescura: Frescura,
    pub truncado: bool,
    pub total_contenedores: usize,
}

/// Construye el snapshot ordenado por nombre con recorte determinista.
pub fn construir(host_id: &str, mut lista: Vec<Contenedor>, edad_ms: u64) -> Snapshot {
    lista.sort_by(|a, b| a.nombre.cmp(&b.nombre));
    let total = lista.len();
    let truncado = total > MAX_CONTENEDORES;
    if truncado {
        lista.truncate(MAX_CONTENEDORES);
    }
    Snapshot {
        schema: SCHEMA_VERSION,
        host_id: host_id.to_owned(),
        ts: ahora_ms(),
        contenedores: lista,
        frescura: Frescura {
            fuente: "fresco",
            edad_ms,
        },
        truncado,
        total_contenedores: total,
    }
}

pub fn ahora_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
