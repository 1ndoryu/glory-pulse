//! Muestreadores: eventos (marca sucio) + stats cada 10s + meta Coolify cada 60s.
//!
//! [por qué] La ruta caliente (`/snapshot`) nunca toca Docker ni red: solo lee
//! este estado. Convergencia: sampler 10s + poll WM 5s ⇒ evento→banner ≤ 15s.

use bollard::Docker;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::{interval, timeout};
use tracing::{debug, info, warn};

use crate::snapshot::{Contenedor, DiscoHost, EstadoContenedor, Recursos};

const INTERVALO_STATS: Duration = Duration::from_secs(10);
const INTERVALO_META: Duration = Duration::from_secs(60);
const TIMEOUT_LLAMADA: Duration = Duration::from_secs(5);
/// Fuente del IO del host; anulable por env para pruebas (`DISKSTATS_PATH`).
const RUTA_DISKSTATS: &str = "/proc/diskstats";

/// Muestra previa de CPU para calcular el % por delta (el one-shot no trae base).
#[derive(Debug, Clone, Copy)]
pub(crate) struct MuestraCpu {
    total: u64,
    sistema: u64,
    ncpu: u32,
}

#[derive(Debug, Clone)]
pub struct Entrada {
    pub contenedor: Contenedor,
    prev: Option<MuestraCpu>,
}

/// Fila de meta lenta Coolify: (sitio_uuid, dominio). Best-effort.
pub type MetaFila = (Option<String>, Option<String>);

#[derive(Debug)]
pub struct EstadoCompartido {
    /// id12 → entrada. Solo lo escriben los muestreadores; lo lee `/snapshot`.
    pub contenedores: RwLock<HashMap<String, Entrada>>,
    /// nombre contenedor → (sitio_uuid, dominio). Best-effort, vacío sin token.
    pub meta: RwLock<HashMap<String, MetaFila>>,
    /// [0110A-1] IO del host agregado. Lo escribe el sampler de disco cada
    /// ciclo; lo lee `/snapshot`. Conserva el último bueno ante fallos.
    pub disco: RwLock<DiscoHost>,
    /// Lo pone el stream de eventos; adelanta el siguiente ciclo de stats.
    pub sucio: AtomicBool,
    pub inicio: Instant,
}

impl Default for EstadoCompartido {
    fn default() -> Self {
        Self {
            contenedores: RwLock::new(HashMap::new()),
            meta: RwLock::new(HashMap::new()),
            disco: RwLock::new(DiscoHost::default()),
            sucio: AtomicBool::new(false),
            inicio: Instant::now(),
        }
    }
}

impl EstadoCompartido {
    pub fn nuevo() -> Arc<Self> {
        Arc::new(Self {
            inicio: Instant::now(),
            ..Default::default()
        })
    }
}

fn como_u64(v: &Value, ptr: &str) -> u64 {
    v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0)
}

fn como_str(v: &Value, ptr: &str) -> String {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// [0110A-1] ¿Disco físico entero? Solo se agregan discos enteros (las
/// particiones contarían doble con su disco). Reglas sin regex:
/// - fuera: `loop*`, `ram*`, `zram*`, `dm-*`, `md*`, `sr*`, `fd*`.
/// - dentro: `sd/hd/vd/xvd` + letras (`sda`, no `sda1`); `nvmeNpM`
///   (`nvme0n1`, no `nvme0n1p1`); `mmcblkN` (`mmcblk0`, no `mmcblk0p1`).
/// - lo demás se ignora (fail-closed: mejor subcontar que inventar).
fn es_disco_fisico(nombre: &str) -> bool {
    for p in ["loop", "ram", "zram", "dm-", "md", "sr", "fd"] {
        if nombre.starts_with(p) {
            return false;
        }
    }
    for p in ["sd", "hd", "vd", "xvd"] {
        if let Some(resto) = nombre.strip_prefix(p) {
            return !resto.is_empty() && resto.bytes().all(|b| b.is_ascii_lowercase());
        }
    }
    if let Some(resto) = nombre.strip_prefix("nvme") {
        // `0n1`: dígitos + 'n' + dígitos, sin 'p' de partición.
        let mut partes = resto.split('n');
        let (a, b) = (partes.next().unwrap_or(""), partes.next().unwrap_or(""));
        return !a.is_empty()
            && !b.is_empty()
            && partes.next().is_none()
            && !b.contains('p')
            && a.bytes().all(|c| c.is_ascii_digit())
            && b.bytes().all(|c| c.is_ascii_digit());
    }
    if let Some(resto) = nombre.strip_prefix("mmcblk") {
        return !resto.is_empty() && resto.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// [0110A-1] Agrega `/proc/diskstats`: sectores leídos (campo 6) y escritos
/// (campo 10) de discos físicos. Función pura sobre el texto para probarla
/// sin `/proc` (`#[cfg(test)]` abajo + fixture en contrato). Basura =
/// ceros, nunca pánico (best-effort como la meta Coolify).
pub fn agregar_diskstats(contenido: &str) -> DiscoHost {
    let mut d = DiscoHost::default();
    for linea in contenido.lines() {
        let c: Vec<&str> = linea.split_whitespace().collect();
        if c.len() < 14 || !es_disco_fisico(c[2]) {
            continue;
        }
        d.sectores_leidos = d.sectores_leidos.saturating_add(c[5].parse().unwrap_or(0));
        d.sectores_escritos = d
            .sectores_escritos
            .saturating_add(c[9].parse().unwrap_or(0));
    }
    d
}

/// Lee y agrega; `None` si el fichero no se puede leer (el sampler
/// conserva el último bueno). En Windows local siempre es `None`.
pub fn leer_disco_host(ruta: &str) -> Option<DiscoHost> {
    std::fs::read_to_string(ruta)
        .ok()
        .map(|t| agregar_diskstats(&t))
}

fn ruta_diskstats() -> String {
    std::env::var("DISKSTATS_PATH").unwrap_or_else(|_| RUTA_DISKSTATS.to_owned())
}

/// Un ciclo de disco: lee `/proc/diskstats` y guarda el agregado.
/// Best-effort: ante cualquier error conserva lo anterior (jamás falla
/// el servicio por el IO).
async fn ciclo_disco(estado: &EstadoCompartido) {
    if let Some(d) = leer_disco_host(&ruta_diskstats()) {
        *estado.disco.write().await = d;
    }
}

/// Convierte un `stats` (one-shot) a `Recursos` con el % por delta.
/// Función pura para poder probarla sin demonio (`tests/contrato.rs`).
pub fn recursos_desde_stats(stats: &Value, anterior: Option<MuestraCpu>) -> (Recursos, MuestraCpu) {
    let total = como_u64(stats, "/cpu_stats/cpu_usage/total_usage");
    let sistema = como_u64(stats, "/cpu_stats/system_cpu_usage");
    let ncpu = stats
        .pointer("/cpu_stats/online_cpus")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .or_else(|| {
            stats
                .pointer("/cpu_stats/cpu_usage/percpu_usage")
                .and_then(Value::as_array)
                .map(|a| a.len() as u32)
        })
        .unwrap_or(1)
        .max(1);
    let muestra = MuestraCpu {
        total,
        sistema,
        ncpu,
    };
    let cpu_pct = match anterior {
        Some(p) if sistema > p.sistema && p.ncpu > 0 => {
            (total.saturating_sub(p.total)) as f64 / (sistema - p.sistema) as f64
                * ncpu as f64
                * 100.0
        }
        _ => 0.0,
    };
    let (mut blk_ro, mut blk_wo) = (0u64, 0u64);
    if let Some(lista) = stats
        .pointer("/blkio_stats/io_service_bytes_recursive")
        .and_then(Value::as_array)
    {
        for e in lista {
            match e.pointer("/op").and_then(Value::as_str) {
                Some("Read") => blk_ro += e.pointer("/value").and_then(Value::as_u64).unwrap_or(0),
                Some("Write") => blk_wo += e.pointer("/value").and_then(Value::as_u64).unwrap_or(0),
                _ => {}
            }
        }
    }
    let (mut net_rx, mut net_tx) = (0u64, 0u64);
    if let Some(redes) = stats.pointer("/networks").and_then(Value::as_object) {
        for (_, r) in redes {
            net_rx += r.pointer("/rx_bytes").and_then(Value::as_u64).unwrap_or(0);
            net_tx += r.pointer("/tx_bytes").and_then(Value::as_u64).unwrap_or(0);
        }
    }
    (
        Recursos {
            cpu_pct,
            mem_usada: como_u64(stats, "/memory_stats/usage"),
            mem_limite: como_u64(stats, "/memory_stats/limit"),
            blk_ro,
            blk_wo,
            net_rx,
            net_tx,
        },
        muestra,
    )
}

fn entrada_desde_inspect(
    id12: &str,
    nombre: &str,
    imagen: &str,
    insp: &Value,
    recursos: Recursos,
    meta: &HashMap<String, (Option<String>, Option<String>)>,
) -> Contenedor {
    let estado = como_str(insp, "/State/Status");
    let salud = insp
        .pointer("/State/Health/Status")
        .and_then(Value::as_str)
        .unwrap_or("sin-chequeo")
        .to_owned();
    let (sitio_uuid, dominio) = meta.get(nombre).cloned().unwrap_or((None, None));
    let mut puertos: Vec<String> = Vec::new();
    if let Some(mapa) = insp
        .pointer("/NetworkSettings/Ports")
        .and_then(Value::as_object)
    {
        for (privado, ligas) in mapa {
            match ligas {
                Value::Array(ligas) => {
                    for l in ligas {
                        let ip = l.pointer("/HostIp").and_then(Value::as_str).unwrap_or("");
                        let pub_ = l.pointer("/HostPort").and_then(Value::as_str).unwrap_or("");
                        puertos.push(format!("{ip}:{pub_}->{privado}"));
                    }
                }
                _ => puertos.push(privado.clone()),
            }
        }
        puertos.sort();
    }
    Contenedor {
        id12: id12.to_owned(),
        nombre: nombre.to_owned(),
        sitio_uuid,
        dominio,
        imagen: imagen.to_owned(),
        estado: EstadoContenedor {
            estado: if estado.is_empty() {
                "desconocido".into()
            } else {
                estado
            },
            salud,
            reinicios: insp
                .pointer("/RestartCount")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            desde: como_str(insp, "/State/StartedAt"),
        },
        recursos,
        puertos,
    }
}

/// Un ciclo de stats: disco del host + lista + inspect + stats por contenedor, poda los idos.
async fn ciclo_stats(docker: &Docker, estado: &EstadoCompartido) {
    ciclo_disco(estado).await;
    let lista = match timeout(
        TIMEOUT_LLAMADA,
        docker.list_containers(Some(bollard::query_parameters::ListContainersOptions {
            all: true,
            ..Default::default()
        })),
    )
    .await
    {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => {
            warn!("list falló: {e}");
            return;
        }
        Err(_) => {
            warn!("list agotó timeout");
            return;
        }
    };
    let meta = estado.meta.read().await;
    let mut vistos = Vec::with_capacity(lista.len());
    let mut mapa = estado.contenedores.write().await;
    for resumen in &lista {
        let Some(id) = resumen.id.as_deref() else {
            continue;
        };
        let id12 = id.get(..12).unwrap_or(id).to_owned();
        let nombre = resumen
            .names
            .as_deref()
            .and_then(|n| n.first())
            .map(|n| n.trim_start_matches('/').to_owned())
            .unwrap_or_else(|| id12.clone());
        vistos.push(id12.clone());
        let insp: Value = match timeout(TIMEOUT_LLAMADA, docker.inspect_container(id, None)).await {
            Ok(Ok(r)) => match serde_json::to_value(&r) {
                Ok(v) => v,
                Err(_) => continue,
            },
            _ => continue,
        };
        let prev = mapa.get(&id12).and_then(|e| e.prev);
        let por_defecto = || {
            (
                Recursos::default(),
                prev.unwrap_or(MuestraCpu {
                    total: 0,
                    sistema: 0,
                    ncpu: 1,
                }),
            )
        };
        let mut corriente = docker.stats(
            id,
            Some(bollard::query_parameters::StatsOptions {
                stream: false,
                one_shot: true,
            }),
        );
        let (recursos, muestra) = match timeout(TIMEOUT_LLAMADA, corriente.next()).await {
            Ok(Some(Ok(st))) => {
                let v = serde_json::to_value(&st).unwrap_or(Value::Null);
                recursos_desde_stats(&v, prev)
            }
            _ => por_defecto(),
        };
        let contenedor = entrada_desde_inspect(
            &id12,
            &nombre,
            resumen.image.as_deref().unwrap_or(""),
            &insp,
            recursos,
            &meta,
        );
        mapa.insert(
            id12,
            Entrada {
                contenedor,
                prev: Some(muestra),
            },
        );
    }
    mapa.retain(|k, _| vistos.contains(k));
    debug!("stats: {} contenedores", mapa.len());
}

/// Stream de eventos: solo marca `sucio` para adelantar el ciclo de stats.
pub async fn bucle_eventos(docker: Docker, estado: Arc<EstadoCompartido>) {
    loop {
        let corriente = docker.events(None::<bollard::query_parameters::EventsOptions>);
        tokio::pin!(corriente);
        while let Some(ev) = corriente.next().await {
            match ev {
                Ok(m) => {
                    let v: Value = serde_json::to_value(&m).unwrap_or(Value::Null);
                    let accion = v
                        .pointer("/action")
                        .or_else(|| v.pointer("/status"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if matches!(
                        accion,
                        "die"
                            | "stop"
                            | "start"
                            | "destroy"
                            | "create"
                            | "rename"
                            | "update"
                            | "health_status"
                    ) {
                        estado.sucio.store(true, Ordering::Relaxed);
                        debug!("evento docker: {accion}");
                    }
                }
                Err(e) => {
                    warn!("eventos: {e}; reintentando en 5s");
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub async fn bucle_stats(docker: Docker, estado: Arc<EstadoCompartido>) {
    let mut tick = interval(INTERVALO_STATS);
    loop {
        tick.tick().await;
        if estado.sucio.swap(false, Ordering::Relaxed) {
            debug!("ciclo adelantado por evento");
        }
        ciclo_stats(&docker, &estado).await;
    }
}

/// Meta Coolify (best-effort): uuid+dominio por nombre. Vacío sin token.
/// Nunca falla el servicio: ante cualquier error, conserva lo anterior.
pub async fn bucle_meta(
    estado: Arc<EstadoCompartido>,
    base: Option<String>,
    token: Option<String>,
) {
    let (Some(base), Some(token)) = (base, token) else {
        info!("meta Coolify desactivada (sin token)");
        return;
    };
    let cliente = match reqwest::Client::builder().timeout(TIMEOUT_LLAMADA).build() {
        Ok(c) => c,
        Err(e) => {
            warn!("meta: sin cliente http ({e})");
            return;
        }
    };
    let url = format!("{}/api/v1/applications", base.trim_end_matches('/'));
    let mut tick = interval(INTERVALO_META);
    loop {
        tick.tick().await;
        match cliente.get(&url).bearer_auth(&token).send().await {
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => {
                    let mut mapa = HashMap::new();
                    let apps = v.pointer("/data").unwrap_or(&v);
                    if let Some(lista) = apps.as_array() {
                        for a in lista {
                            let uuid = a.pointer("/uuid").and_then(Value::as_str).unwrap_or("");
                            let nombre = a.pointer("/name").and_then(Value::as_str).unwrap_or("");
                            let dominio = a
                                .pointer("/fqdn")
                                .or_else(|| a.pointer("/domains"))
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                            if !uuid.is_empty() && !nombre.is_empty() {
                                // [por qué] Sin labels en el manager (F0), el
                                // enlace es best-effort por nombre exacto.
                                mapa.insert(nombre.to_owned(), (Some(uuid.to_owned()), dominio));
                            }
                        }
                    }
                    *estado.meta.write().await = mapa;
                    debug!("meta:adonada");
                }
                Err(e) => warn!("meta: json inesperado ({e})"),
            },
            Err(e) => warn!("meta: {e}"),
        }
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    // Formato real del kernel: mayor menor nombre + 11 campos (mínimo 14).
    const DISKSTATS: &str = "   8       0 sda 100 10 3000 40 50 5 7000 60 0 0 0 0 0 0\n\
         8       1 sda1 20 2 500 8 10 1 900 9 0 0 0 0 0 0\n\
         8      16 sdb 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n\
       259       0 nvme0n1 7 0 800 1 3 0 400 1 0 0 0 0 0 0\n\
       259       1 nvme0n1p1 7 0 800 1 3 0 400 1 0 0 0 0 0 0\n\
       179       0 mmcblk0 1 0 100 0 1 0 200 0 0 0 0 0 0 0\n\
         7       0 loop0 5 0 600 0 0 0 0 0 0 0 0 0 0 0\n\
         1       0 ram0 9 0 999 0 9 0 888 0 0 0 0 0 0 0\n\
       253       0 dm-0 4 0 111 0 4 0 222 0 0 0 0 0 0 0\n\
       basura sin campos\n";

    #[test]
    fn disco_fisico_solo_enteros() {
        for d in ["sda", "sdb", "hda", "vda", "xvda", "nvme0n1", "mmcblk0"] {
            assert!(es_disco_fisico(d), "{d} es disco entero");
        }
        for d in [
            "sda1",
            "nvme0n1p1",
            "mmcblk0p1",
            "loop0",
            "ram0",
            "zram0",
            "dm-0",
            "md0",
            "sr0",
            "fd0",
            "",
            "nada",
        ] {
            assert!(!es_disco_fisico(d), "{d} no agrega");
        }
    }

    #[test]
    fn diskstats_agrega_enteros_y_excluye_resto() {
        // sda 3000/7000 + sdb 0/0 + nvme0n1 800/400 + mmcblk0 100/200.
        let d = agregar_diskstats(DISKSTATS);
        assert_eq!(d.sectores_leidos, 3900);
        assert_eq!(d.sectores_escritos, 7600);
    }

    #[test]
    fn diskstats_basura_da_ceros() {
        let d = agregar_diskstats("basura\n\n   1 2 corto\n");
        assert_eq!(d.sectores_leidos, 0);
        assert_eq!(d.sectores_escritos, 0);
    }

    #[test]
    fn disco_host_sin_fichero_es_none() {
        assert!(leer_disco_host("/ruta/que/no/existe/diskstats").is_none());
    }
}
