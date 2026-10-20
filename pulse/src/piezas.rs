//! Piezas del detalle por proyecto (19AA-3): los equivalentes de las que el tablero pide a
//! Coolify/Docker en la VPS 66, pero leídos del taller de la VPS 89.
//!
//! [por qué] Aquí sí se lee `/proc` en cada petición (no como `/snapshot`, que solo toca
//! memoria): son piezas a demanda, y lo que se quiere es el estado de ahora mismo. Sigue
//! prohibido exec y spawn; el único acceso a red es la sonda HTTP a loopback del propio
//! despliegue, que es lo que la pieza `salud` mide.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::procesos::{
    Evento, Proc, escuchas_del_host, hilos_de_status, io_de_proc, leer_procesos, puertos_de,
    rss_de_status,
};

/// Techo de la pieza `logs`: ni más bytes por lectura ni más líneas por petición.
const MAX_BYTES_LOG: u64 = 256 * 1024;
pub const MAX_LINEAS_LOG: usize = 500;
/// Ventana entre las dos lecturas de `/proc` de la pieza `stats` (el % sale por delta).
const VENTANA_CPU: Duration = Duration::from_millis(250);

pub const PIEZAS: [&str; 7] = [
    "salud",
    "stats",
    "inspeccion",
    "eventos",
    "bd",
    "diagnostico",
    "logs",
];

/// Nombre de proyecto válido (puro, sin tocar el disco): un solo segmento, sin `..` ni
/// separadores, y nunca una carpeta interna (`_logs`, `_datos`). Fail-closed.
pub fn es_sitio_valido(sitio: &str) -> bool {
    !sitio.is_empty()
        && sitio.len() <= 128
        && !sitio.starts_with('_')
        && !sitio.starts_with('.')
        && sitio
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'.'))
}

/// Ruta del proyecto si `sitio` es un nombre válido y existe como carpeta.
/// [por qué] `es_sitio_valido` ya prohíbe separadores, pero el nombre llega por HTTP: se
/// canonicaliza y se exige el prefijo de la raíz, para que ni un enlace del taller saque la
/// ruta de su carpeta.
pub fn ruta_proyecto(raiz: &str, sitio: &str) -> Option<PathBuf> {
    if !es_sitio_valido(sitio) {
        return None;
    }
    let raiz_real = fs::canonicalize(raiz).ok()?;
    let ruta = raiz_real.join(sitio);
    let real = fs::canonicalize(&ruta).ok()?;
    (real.is_dir() && real.starts_with(&raiz_real)).then_some(ruta)
}

/// Procesos del proyecto (semilla por `cwd` + descendientes por PPID).
pub fn procesos_de(raiz: &str, sitio: &str) -> Vec<Proc> {
    let procs = leer_procesos();
    let grupos = crate::procesos::agrupar(&procs, raiz);
    let Some(idxs) = grupos.get(sitio) else {
        return Vec::new();
    };
    idxs.iter().map(|i| procs[*i].clone()).collect()
}

fn usuario_de(uid: &str) -> String {
    let Ok(passwd) = fs::read_to_string("/etc/passwd") else {
        return uid.to_owned();
    };
    passwd
        .lines()
        .filter_map(|l| {
            let mut c = l.split(':');
            let nombre = c.next()?;
            c.next()?;
            (c.next()? == uid).then(|| nombre.to_owned())
        })
        .next()
        .unwrap_or_else(|| uid.to_owned())
}

fn uid_de(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|v| v.split_whitespace().next().map(str::to_owned))
        })
        .unwrap_or_default()
}

/// Últimas `n` líneas de un fichero leyendo solo su cola (el log del taller crece).
pub fn cola_de_fichero(ruta: &Path, n: usize) -> Option<(String, u64)> {
    let mut f = File::open(ruta).ok()?;
    let bytes = f.metadata().ok()?.len();
    let desde = bytes.saturating_sub(MAX_BYTES_LOG);
    f.seek(SeekFrom::Start(desde)).ok()?;
    let mut bruto = Vec::new();
    f.read_to_end(&mut bruto).ok()?;
    let texto = String::from_utf8_lossy(&bruto).into_owned();
    let lineas: Vec<&str> = texto.lines().collect();
    // [por qué] Si se recortó por bytes, la primera línea suele estar a medias: se tira.
    let recorte = if desde > 0 { 1 } else { 0 };
    let desde_linea = lineas.len().saturating_sub(n + recorte);
    Some((lineas[desde_linea..].join("\n"), bytes))
}

/// Fichero de log de la sesión del proyecto: el `dev-up-<id>-*.log` más reciente.
/// La carpeta se canonicaliza y se comprueba el prefijo, como en `ruta_proyecto`.
pub fn log_del_proyecto(raiz: &str, sitio: &str) -> Option<PathBuf> {
    if !es_sitio_valido(sitio) {
        return None;
    }
    let base = fs::canonicalize(Path::new(raiz).join("_logs")).ok()?;
    let dir = base.join(sitio);
    let real = fs::canonicalize(&dir).ok()?;
    if !real.starts_with(&base) {
        return None;
    }
    let mut mejores: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let nombre = e.file_name().to_string_lossy().into_owned();
            if !nombre.starts_with("dev-up-") || !nombre.ends_with(".log") {
                return None;
            }
            let modif = e.metadata().ok()?.modified().ok()?;
            Some((modif, e.path()))
        })
        .collect();
    mejores.sort();
    mejores.pop().map(|(_, p)| p)
}

fn proc_a_json(p: &Proc, escuchas: &HashMap<u64, crate::procesos::Escucha>) -> Value {
    let status = fs::read_to_string(format!("/proc/{}/status", p.pid)).unwrap_or_default();
    let uid = uid_de(p.pid);
    let (io_r, io_w) = fs::read_to_string(format!("/proc/{}/io", p.pid))
        .map(|t| io_de_proc(&t))
        .unwrap_or((0, 0));
    json!({
        "pid": p.pid,
        "ppid": p.ppid,
        "usuario": usuario_de(&uid),
        // [por qué] La cmdline del taller trae el entorno entero (`export PATH=… DATABASE_URL=…`):
        // recortada cabe en el panel y sigue diciendo qué es el proceso.
        "cmdline": p.cmdline.chars().take(crate::procesos::MAX_CMDLINE).collect::<String>(),
        "cwd": p.cwd,
        "rssMiB": rss_de_status(&status) / (1024 * 1024),
        "hilos": hilos_de_status(&status),
        "ioLeidoBytes": io_r,
        "ioEscritoBytes": io_w,
        "puertos": puertos_de(&[p.pid], escuchas),
    })
}

/// Comprueba que el despliegue responde en su primer puerto de loopback.
async fn sonda_http(cliente: &reqwest::Client, puertos: &[String]) -> Value {
    let Some(puerto) = puertos
        .iter()
        .filter_map(|p| p.rsplit(':').next())
        .find(|p| !p.is_empty())
    else {
        return Value::Null;
    };
    let url = format!("http://127.0.0.1:{puerto}/");
    let t0 = Instant::now();
    match cliente.get(&url).send().await {
        Ok(r) => json!({
            "url": url,
            "status": r.status().as_u16(),
            "ms": t0.elapsed().as_millis() as u64,
        }),
        Err(e) => json!({
            "url": url,
            "status": Value::Null,
            "ms": t0.elapsed().as_millis() as u64,
            "error": e.to_string().chars().take(120).collect::<String>(),
        }),
    }
}

/// Memoria y swap del host desde `/proc/meminfo` (en MiB).
fn meminfo() -> Value {
    let txt = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mib = |clave: &str| {
        txt.lines()
            .find_map(|l| l.strip_prefix(clave))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            / 1024
    };
    let total = mib("MemTotal:");
    let disponible = mib("MemAvailable:");
    json!({
        "memTotalMiB": total,
        "memDisponibleMiB": disponible,
        "memUsadaMiB": total.saturating_sub(disponible),
        "swapTotalMiB": mib("SwapTotal:"),
        "swapLibreMiB": mib("SwapFree:"),
    })
}

fn carga() -> Vec<f64> {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|t| {
            t.split_whitespace()
                .take(3)
                .filter_map(|v| v.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Una pieza del detalle. `Ok` = datos; `Err` = código corto (el tablero lo muestra tal cual).
/// [por qué] Despachador fino: cada pieza vive en su propia función, que es lo que mantiene
/// esto por debajo del límite de líneas del gate (`funcion-larga-rs`).
pub async fn pieza(
    nombre: &str,
    raiz: &str,
    sitio: &str,
    lineas: usize,
    eventos: &[Evento],
    cliente: &reqwest::Client,
) -> Result<Value, String> {
    let Some(ruta) = ruta_proyecto(raiz, sitio) else {
        return Err("proyecto-desconocido".into());
    };
    match nombre {
        "salud" => Ok(pieza_salud(raiz, sitio, cliente).await),
        "stats" => Ok(pieza_stats(raiz, sitio).await),
        "inspeccion" => Ok(pieza_inspeccion(raiz, sitio, &ruta)),
        "eventos" => Ok(pieza_eventos(sitio, eventos)),
        "bd" => Ok(pieza_bd()),
        "diagnostico" => Ok(pieza_diagnostico(raiz, sitio)),
        "logs" => pieza_logs(raiz, sitio, lineas),
        _ => Err("pieza-desconocida".into()),
    }
}

/// `salud`: ¿está vivo y contesta? Procesos, puertos, sonda HTTP a loopback y log de la sesión.
async fn pieza_salud(raiz: &str, sitio: &str, cliente: &reqwest::Client) -> Value {
    let procs = procesos_de(raiz, sitio);
    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let puertos = puertos_de(&pids, &escuchas_del_host());
    json!({
        "vivo": !procs.is_empty(),
        "procesos": procs.len(),
        "puertos": puertos,
        "http": sonda_http(cliente, &puertos).await,
        "log": log_del_proyecto(raiz, sitio).map(|p| p.to_string_lossy().into_owned()),
    })
}

/// `stats`: CPU y memoria por proceso. Dos barridos separados por `VENTANA_CPU`: sin delta no
/// hay % de CPU.
async fn pieza_stats(raiz: &str, sitio: &str) -> Value {
    let antes: HashMap<u32, (u64, u64)> = leer_procesos()
        .iter()
        .map(|p| (p.pid, (p.utime, p.stime)))
        .collect();
    let j0 = fs::read_to_string("/proc/stat")
        .map(|t| crate::procesos::jiffies_totales(&t))
        .unwrap_or(0);
    tokio::time::sleep(VENTANA_CPU).await;
    let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
    let j1 = crate::procesos::jiffies_totales(&stat);
    let ncpu = crate::procesos::ncpu_de_stat(&stat);
    let procs = procesos_de(raiz, sitio);
    let escuchas = escuchas_del_host();
    let dtotal = j1.saturating_sub(j0).max(1);
    let filas: Vec<Value> = procs
        .iter()
        .map(|p| {
            let dproc = antes
                .get(&p.pid)
                .map(|(u, s)| (p.utime + p.stime).saturating_sub(u + s))
                .unwrap_or(0);
            let mut v = proc_a_json(p, &escuchas);
            v["cpuPct"] = json!(dproc as f64 / dtotal as f64 * f64::from(ncpu) * 100.0);
            v
        })
        .collect();
    json!({
        "procesos": filas,
        "cpuPct": filas.iter().filter_map(|f| f["cpuPct"].as_f64()).sum::<f64>(),
        "memUsadaMiB": procs.iter().map(|p| p.rss).sum::<u64>() / (1024 * 1024),
    })
}

/// `inspeccion`: la foto fija de cada proceso del proyecto, con su árbol de hijos.
fn pieza_inspeccion(raiz: &str, sitio: &str, ruta: &Path) -> Value {
    let procs = procesos_de(raiz, sitio);
    let escuchas = escuchas_del_host();
    let hijos: HashMap<u32, Vec<u32>> = {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for p in &procs {
            m.entry(p.ppid).or_default().push(p.pid);
        }
        m
    };
    json!({
        "proyecto": sitio,
        "ruta": ruta.to_string_lossy(),
        "procesos": procs.iter().map(|p| {
            let mut v = proc_a_json(p, &escuchas);
            v["hijos"] = json!(hijos.get(&p.pid).cloned().unwrap_or_default());
            v
        }).collect::<Vec<_>>(),
    })
}

/// `eventos`: lo que pulse vio del proyecto (arranques, caídas) en memoria.
fn pieza_eventos(sitio: &str, eventos: &[Evento]) -> Value {
    json!({
        "proyecto": sitio,
        "eventos": eventos.iter().filter(|e| e.proyecto == sitio).collect::<Vec<_>>(),
    })
}

/// `bd`: la base de datos del taller (PostgreSQL del host): escuchas en 5432 y sockets locales.
fn pieza_bd() -> Value {
    let escuchas = escuchas_del_host();
    let pg: Vec<String> = escuchas
        .values()
        .filter(|e| e.puerto == 5432)
        .map(|e| format!("{}:{}", e.ip, e.puerto))
        .collect();
    let dir = Path::new("/var/run/postgresql");
    let sockets: Vec<String> = fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".s.PGSQL."))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "escuchando": !pg.is_empty(),
        "escuchas": pg,
        "dirSockets": dir.to_string_lossy(),
        "sockets": sockets,
        "accesible": dir.is_dir(),
    })
}

/// `diagnostico`: memoria y carga del host, más el estado del taller (qué proyectos no escuchan).
fn pieza_diagnostico(raiz: &str, sitio: &str) -> Value {
    let procs = leer_procesos();
    let grupos = crate::procesos::agrupar(&procs, raiz);
    let escuchas = escuchas_del_host();
    let sin_puerto: Vec<String> = grupos
        .iter()
        .filter(|(_, idxs)| {
            let pids: Vec<u32> = idxs.iter().map(|i| procs[*i].pid).collect();
            puertos_de(&pids, &escuchas).is_empty()
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut mem = meminfo();
    mem["carga"] = json!(carga());
    mem["proyectosDelTaller"] = json!(grupos.len());
    mem["proyectosSinPuerto"] = json!(sin_puerto);
    mem["logDelProyecto"] = match log_del_proyecto(raiz, sitio) {
        Some(p) => json!({
            "ruta": p.to_string_lossy(),
            "bytes": fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
        }),
        None => Value::Null,
    };
    mem
}

/// `logs`: las últimas `lineas` del log de la sesión del proyecto.
fn pieza_logs(raiz: &str, sitio: &str, lineas: usize) -> Result<Value, String> {
    let n = lineas.clamp(1, MAX_LINEAS_LOG);
    let Some(ruta_log) = log_del_proyecto(raiz, sitio) else {
        return Err("sin-log".into());
    };
    let Some((texto, bytes)) = cola_de_fichero(&ruta_log, n) else {
        return Err("log-ilegible".into());
    };
    Ok(json!({
        "fichero": ruta_log.to_string_lossy(),
        "bytes": bytes,
        "lineas": n,
        "texto": texto,
    }))
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn tmp(nombre: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pulse-pruebas-{nombre}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("tmp");
        d
    }

    #[test]
    fn ruta_proyecto_fail_closed() {
        let raiz = tmp("ruta");
        fs::create_dir_all(raiz.join("NAKOMI")).unwrap();
        fs::create_dir_all(raiz.join("_logs")).unwrap();
        let r = raiz.to_string_lossy();
        assert!(ruta_proyecto(&r, "NAKOMI").is_some());
        assert!(ruta_proyecto(&r, "no-existe").is_none());
        assert!(ruta_proyecto(&r, "_logs").is_none());
        assert!(ruta_proyecto(&r, "..").is_none());
        assert!(ruta_proyecto(&r, "a/b").is_none());
        assert!(ruta_proyecto(&r, "").is_none());
    }

    #[test]
    fn cola_de_log_toma_las_ultimas_lineas() {
        let d = tmp("cola");
        let f = d.join("dev-up-X-1.log");
        fs::write(&f, "uno\ndos\ntres\ncuatro\n").unwrap();
        let (texto, bytes) = cola_de_fichero(&f, 2).expect("cola");
        assert_eq!(texto, "tres\ncuatro");
        assert_eq!(bytes, 20);
        assert!(cola_de_fichero(&d.join("no-existe.log"), 2).is_none());
    }

    #[test]
    fn log_del_proyecto_elige_el_mas_reciente() {
        let raiz = tmp("logs");
        let dir = raiz.join("_logs").join("NAKOMI");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("dev-up-NAKOMI-3103.log"), "viejo").unwrap();
        fs::write(dir.join("npm-ci-NAKOMI-frontend.log"), "otro").unwrap();
        fs::write(dir.join("dev-up-NAKOMI-5176.log"), "nuevo").unwrap();
        let r = raiz.to_string_lossy();
        let elegido = log_del_proyecto(&r, "NAKOMI").expect("log");
        assert!(elegido.ends_with("dev-up-NAKOMI-5176.log"), "{elegido:?}");
        assert!(log_del_proyecto(&r, "OTRO").is_none());
    }

    #[test]
    fn meminfo_sale_en_mib() {
        let v = meminfo();
        assert!(v["memTotalMiB"].as_u64().unwrap_or(0) > 0);
        assert!(v["memDisponibleMiB"].is_u64());
        assert!(v["swapTotalMiB"].is_u64());
    }
}
