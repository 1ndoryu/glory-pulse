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
    /// [309A-2] Imagen del `list` (gratis, a memoria). Vacía = desconocida
    /// y no se serializa (contrato viejo intacto).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub imagen: String,
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

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoHost {
    /// Sectores leídos acumulados del host (todos los discos físicos).
    pub sectores_leidos: u64,
    /// Sectores escritos acumulados del host.
    pub sectores_escritos: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema: u32,
    pub host_id: String,
    pub ts: u64,
    pub contenedores: Vec<Contenedor>,
    /// [0110A-1] IO del host (`/proc/diskstats` agregado). Acumulados como
    /// `netRX/netTX`: la velocidad la calcula quien consume (delta/dt).
    /// Siempre presente (ceros donde `/proc` no se puede leer).
    pub disco_host: DiscoHost,
    pub frescura: Frescura,
    pub truncado: bool,
    pub total_contenedores: usize,
}

/// Construye el snapshot ordenado por nombre con recorte determinista.
pub fn construir(
    host_id: &str,
    mut lista: Vec<Contenedor>,
    edad_ms: u64,
    disco: DiscoHost,
) -> Snapshot {
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
        disco_host: disco,
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

/// [309A-2] Detalle por sitio en una sola conexión: subconjunto del
/// snapshot filtrado en memoria (el handler jamás toca Docker).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetalleSitio {
    pub schema: u32,
    pub host_id: String,
    pub ts: u64,
    pub sitio: String,
    pub contenedores: Vec<Contenedor>,
    pub total_contenedores: usize,
}

/// Prefijos Coolify que enlazan contenedor→sitio (misma regla que el
/// manager `extraerUuidContenedor`: sin ella habría que tocar Rust al
/// añadir un workload nuevo).
const PREFIJOS_SITIO: [&str; 5] = ["app", "postgres", "socket-proxy", "mariadb", "wordpress"];

/// Fail-closed: uuid Coolify = alfanumérico no vacío (rechaza `..`, `/`,
/// query raras antes de filtrar).
pub fn es_uuid_valido(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Filtro puro (probable sin demonio): por nombre `<pref>-<uuid>` o por
/// `sitio_uuid` de la meta Coolify cuando exista.
pub fn filtrar_por_sitio(lista: &[Contenedor], uuid: &str) -> Vec<Contenedor> {
    lista
        .iter()
        .filter(|c| {
            c.sitio_uuid.as_deref() == Some(uuid)
                || PREFIJOS_SITIO
                    .iter()
                    .any(|p| c.nombre == format!("{p}-{uuid}"))
        })
        .cloned()
        .collect()
}

/// Construye el detalle ordenado por nombre (sin recorte: un sitio cabe).
pub fn construir_detalle(host_id: &str, sitio: &str, mut lista: Vec<Contenedor>) -> DetalleSitio {
    lista.sort_by(|a, b| a.nombre.cmp(&b.nombre));
    let total = lista.len();
    DetalleSitio {
        schema: SCHEMA_VERSION,
        host_id: host_id.to_owned(),
        ts: ahora_ms(),
        sitio: sitio.to_owned(),
        contenedores: lista,
        total_contenedores: total,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn contenedor(nombre: &str, uuid: Option<&str>) -> Contenedor {
        Contenedor {
            id12: "abc123".into(),
            nombre: nombre.into(),
            sitio_uuid: uuid.map(str::to_owned),
            dominio: None,
            imagen: String::new(),
            estado: EstadoContenedor {
                estado: "running".into(),
                salud: "sin-chequeo".into(),
                reinicios: 0,
                desde: String::new(),
            },
            recursos: Recursos::default(),
            puertos: Vec::new(),
        }
    }

    #[test]
    fn uuid_valido_rechaza_raros() {
        assert!(es_uuid_valido("r4okw44w84c0ko88g844kosk"));
        assert!(!es_uuid_valido(""));
        assert!(!es_uuid_valido("../x"));
        assert!(!es_uuid_valido("a/b"));
        assert!(!es_uuid_valido("a b"));
    }

    #[test]
    fn filtro_por_nombre_y_por_meta() {
        let lista = vec![
            contenedor("app-AAA", None),
            contenedor("postgres-AAA", None),
            contenedor("app-BBB", None),
            contenedor("raro", Some("AAA")),
            contenedor("socket-proxy-infra", None),
        ];
        let got = filtrar_por_sitio(&lista, "AAA");
        assert_eq!(got.len(), 3);
        let got = filtrar_por_sitio(&lista, "ZZZ");
        assert!(got.is_empty());
    }
}
