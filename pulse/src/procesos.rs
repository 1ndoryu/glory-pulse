//! Fuente por procesos (19AA-3): el taller de la VPS 89 no tiene Docker, así que el
//! «contenedor» del snapshot pasa a ser un proyecto del taller leído de `/proc`.
//!
//! [por qué] La regla de `main.rs` prohíbe exec y spawn: aquí todo son lecturas de
//! ficheros. Un proyecto son los procesos cuya `cwd` cuelga de la raíz del taller más sus
//! descendientes por PPID (los hijos no siempre comparten `cwd`). Lo que no está vivo no
//! aparece: las filas de los proyectos parados las pone el tablero, que sí tiene el
//! registro del taller.
//!
//! Los parsers son funciones puras sobre texto, para probarlos sin `/proc`.

use std::collections::{BTreeMap, HashMap};
use std::fs;

use crate::snapshot::{Contenedor, EstadoContenedor, Recursos};

/// Recorte del comando que se publica como `imagen` (la línea cruda trae el entorno).
pub const MAX_CMDLINE: usize = 120;

/// Un proceso de `/proc` reducido a lo que se usa.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub cwd: String,
    pub cmdline: String,
    pub utime: u64,
    pub stime: u64,
    /// `starttime` en jiffies desde el arranque (campo 22 de `stat`).
    pub inicio: u64,
    pub rss: u64,
    pub io_r: u64,
    pub io_w: u64,
}

/// Transición observada por el muestreador (arranque, parada, cambio de puertos).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evento {
    pub ts: u64,
    pub proyecto: String,
    pub texto: String,
}

/// Puerto en escucha de `/proc/net/tcp*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escucha {
    pub ip: String,
    pub puerto: u16,
    pub inodo: u64,
}

/// Campos de `/proc/<pid>/stat` que se usan: ppid, utime, stime, starttime.
/// [por qué] El nombre del proceso va entre paréntesis y puede llevar espacios y
/// paréntesis, así que se parsea desde el ÚLTIMO `)`: a partir de ahí `state` es el campo
/// 3 y los índices son fijos.
pub fn parsear_stat(txt: &str) -> Option<(u32, u64, u64, u64)> {
    let c: Vec<&str> = txt.rsplit_once(')')?.1.split_whitespace().collect();
    if c.len() < 20 {
        return None;
    }
    Some((
        c[1].parse().ok()?,  // ppid
        c[11].parse().ok()?, // utime
        c[12].parse().ok()?, // stime
        c[19].parse().ok()?, // starttime
    ))
}

/// `VmRSS` de `/proc/<pid>/status` en bytes (el fichero lo da en kB).
pub fn rss_de_status(txt: &str) -> u64 {
    campo_status(txt, "VmRSS:").saturating_mul(1024)
}

/// `Threads` de `/proc/<pid>/status`.
pub fn hilos_de_status(txt: &str) -> u64 {
    campo_status(txt, "Threads:")
}

fn campo_status(txt: &str, clave: &str) -> u64 {
    txt.lines()
        .find_map(|l| l.strip_prefix(clave))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// `read_bytes`/`write_bytes` de `/proc/<pid>/io`.
pub fn io_de_proc(txt: &str) -> (u64, u64) {
    let leer = |clave: &str| {
        txt.lines()
            .find_map(|l| l.strip_prefix(clave))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    };
    (leer("read_bytes:"), leer("write_bytes:"))
}

/// Jiffies de CPU de la máquina (primera línea `cpu` de `/proc/stat`).
pub fn jiffies_totales(txt: &str) -> u64 {
    txt.lines()
        .find_map(|l| l.strip_prefix("cpu "))
        .map(|v| {
            v.split_whitespace()
                .filter_map(|x| x.parse::<u64>().ok())
                .sum()
        })
        .unwrap_or(0)
}

/// Nº de CPUs (líneas `cpuN` de `/proc/stat`), con 1 como suelo.
pub fn ncpu_de_stat(txt: &str) -> u32 {
    let n = txt
        .lines()
        .filter(|l| match l.strip_prefix("cpu") {
            // [por qué] El contador va pegado a `cpu` (`cpu0`) y la línea agregada (`cpu  100 …`)
            // lleva espacio: sin esta comprobación el total de la máquina contaría como una CPU más.
            Some(resto) => {
                !resto.starts_with(char::is_whitespace)
                    && resto
                        .split_whitespace()
                        .next()
                        .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            }
            None => false,
        })
        .count();
    (n as u32).max(1)
}

/// IPv4 de `/proc/net/tcp`: cuatro bytes en orden de red escritos como u32 little-endian.
pub fn ipv4_de_hex(hex: &str) -> Option<String> {
    if hex.len() != 8 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(format!(
        "{}.{}.{}.{}",
        v & 0xFF,
        (v >> 8) & 0xFF,
        (v >> 16) & 0xFF,
        (v >> 24) & 0xFF
    ))
}

/// IPv6 de `/proc/net/tcp6`: cuatro palabras de 32 bits, cada una little-endian.
pub fn ipv6_de_hex(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, palabra) in hex.as_bytes().chunks(8).enumerate() {
        let v = u32::from_str_radix(std::str::from_utf8(palabra).ok()?, 16).ok()?;
        bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    Some(
        bytes
            .chunks(2)
            .map(|p| format!("{:02x}{:02x}", p[0], p[1]))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// Entradas en escucha (`st` = `0A`) de `/proc/net/tcp` o `tcp6`.
pub fn parsear_listen(txt: &str, v6: bool) -> Vec<Escucha> {
    let mut out = Vec::new();
    for linea in txt.lines().skip(1) {
        let c: Vec<&str> = linea.split_whitespace().collect();
        if c.len() < 10 || c[3] != "0A" {
            continue;
        }
        let Some((dir_hex, puerto_hex)) = c[1].split_once(':') else {
            continue;
        };
        let ip = if v6 {
            ipv6_de_hex(dir_hex)
        } else {
            ipv4_de_hex(dir_hex)
        };
        let (Some(ip), Ok(puerto), Ok(inodo)) = (
            ip,
            u16::from_str_radix(puerto_hex, 16),
            c[9].parse::<u64>(),
        ) else {
            continue;
        };
        out.push(Escucha { ip, puerto, inodo });
    }
    out
}

/// Proyecto al que pertenece una `cwd`, o `None` si no cuelga de la raíz del taller.
/// [por qué] Las carpetas con `_` o `.` son del propio taller (`_logs`, `_datos`), no
/// proyectos.
pub fn id_proyecto(cwd: &str, raiz: &str) -> Option<String> {
    let resto = cwd.strip_prefix(raiz)?.strip_prefix('/')?;
    let id = resto.split('/').next()?;
    if id.is_empty() || id.starts_with('_') || id.starts_with('.') {
        return None;
    }
    Some(id.to_owned())
}

/// Reparte los procesos entre proyectos: los de `cwd` dentro del taller son semilla y
/// arrastran a sus descendientes (por PPID, iterando hasta que no cambie nada).
pub fn agrupar(procs: &[Proc], raiz: &str) -> BTreeMap<String, Vec<usize>> {
    let mut de_pid: HashMap<u32, usize> = HashMap::with_capacity(procs.len());
    for (i, p) in procs.iter().enumerate() {
        de_pid.insert(p.pid, i);
    }
    let mut grupo: Vec<Option<String>> = procs.iter().map(|p| id_proyecto(&p.cwd, raiz)).collect();
    let mut cambio = true;
    while cambio {
        cambio = false;
        for (i, p) in procs.iter().enumerate() {
            if grupo[i].is_some() {
                continue;
            }
            if let Some(j) = de_pid.get(&p.ppid)
                && let Some(id) = grupo[*j].clone()
            {
                grupo[i] = Some(id);
                cambio = true;
            }
        }
    }
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, g) in grupo.iter().enumerate() {
        if let Some(id) = g {
            out.entry(id.clone()).or_default().push(i);
        }
    }
    out
}

/// `id12` estable y sin colisiones por nombre: FNV-1a en hexadecimal.
pub fn id12(id: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")[..12].to_owned()
}

/// Comando publicado como `imagen`: el del proceso de más memoria (el principal), que es
/// el que dice qué es el despliegue; el del envoltorio de tmux solo trae el entorno.
pub fn comando_principal(procs: &[&Proc]) -> String {
    procs
        .iter()
        .max_by_key(|p| p.rss)
        .map(|p| p.cmdline.trim())
        .unwrap_or("")
        .chars()
        .take(MAX_CMDLINE)
        .collect()
}

fn leer(ruta: &str) -> Option<String> {
    fs::read_to_string(ruta).ok()
}

/// Un barrido de `/proc`: los procesos con `stat` legible (los ajenos no lo son).
pub fn leer_procesos() -> Vec<Proc> {
    let Ok(dir) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let base = format!("/proc/{pid}");
        let Some(stat) = leer(&format!("{base}/stat")) else {
            continue;
        };
        let Some((ppid, utime, stime, inicio)) = parsear_stat(&stat) else {
            continue;
        };
        let cmdline = leer(&format!("{base}/cmdline"))
            .map(|c| c.replace('\0', " ").trim().to_owned())
            .unwrap_or_default();
        let cwd = fs::read_link(format!("{base}/cwd"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = leer(&format!("{base}/status")).unwrap_or_default();
        let (io_r, io_w) = leer(&format!("{base}/io"))
            .map(|t| io_de_proc(&t))
            .unwrap_or((0, 0));
        out.push(Proc {
            pid,
            ppid,
            cwd,
            cmdline,
            utime,
            stime,
            inicio,
            rss: rss_de_status(&status),
            io_r,
            io_w,
        });
    }
    out
}

/// Inodos de socket abiertos por un proceso (`/proc/<pid>/fd` → `socket:[N]`).
pub fn inodos_de(pid: u32) -> Vec<u64> {
    let Ok(dir) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| fs::read_link(e.path()).ok())
        .filter_map(|p| {
            let t = p.to_string_lossy().into_owned();
            let n = t.strip_prefix("socket:[")?.strip_suffix(']')?;
            n.parse().ok()
        })
        .collect()
}

/// Puertos en escucha del host, por inodo (los pids se cruzan con `inodos_de`).
pub fn escuchas_del_host() -> HashMap<u64, Escucha> {
    let mut out = HashMap::new();
    for (ruta, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Some(txt) = leer(ruta) {
            for e in parsear_listen(&txt, v6) {
                out.insert(e.inodo, e);
            }
        }
    }
    out
}

/// Puertos en escucha (`ip:puerto`, ordenados) de un conjunto de procesos.
pub fn puertos_de(pids: &[u32], escuchas: &HashMap<u64, Escucha>) -> Vec<String> {
    let mut out: Vec<String> = pids
        .iter()
        .flat_map(|p| inodos_de(*p))
        .filter_map(|i| escuchas.get(&i).map(|e| format!("{}:{}", e.ip, e.puerto)))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Muestra previa por proceso para el % de CPU por delta (el barrido no trae base).
#[derive(Debug, Clone, Copy)]
struct MuestraProc {
    utime: u64,
    stime: u64,
    inicio: u64,
}

/// Muestreador del taller: guarda la muestra anterior para los deltas de CPU.
pub struct MuestreadorProcesos {
    raiz: String,
    prev: HashMap<u32, MuestraProc>,
    jiffies: u64,
    ncpu: u32,
}

impl MuestreadorProcesos {
    pub fn nuevo(raiz: String) -> Self {
        Self {
            raiz,
            prev: HashMap::new(),
            jiffies: 0,
            ncpu: 1,
        }
    }

    /// Un ciclo: lee `/proc`, agrupa por proyecto y devuelve las filas del snapshot.
    pub fn ciclo(&mut self) -> Vec<Contenedor> {
        let stat = leer("/proc/stat").unwrap_or_default();
        let jiffies = jiffies_totales(&stat);
        self.ncpu = ncpu_de_stat(&stat);
        let djiffies = jiffies.saturating_sub(self.jiffies);
        self.jiffies = jiffies;

        let procs = leer_procesos();
        let grupos = agrupar(&procs, &self.raiz);
        let escuchas = escuchas_del_host();
        let mut nuevas: HashMap<u32, MuestraProc> = HashMap::new();
        let mut out = Vec::with_capacity(grupos.len());
        for (id, idxs) in &grupos {
            let mut cpu = 0.0;
            let mut rss = 0u64;
            let (mut io_r, mut io_w) = (0u64, 0u64);
            let mut del_grupo: Vec<&Proc> = Vec::with_capacity(idxs.len());
            for i in idxs {
                let p = &procs[*i];
                del_grupo.push(p);
                nuevas.insert(
                    p.pid,
                    MuestraProc {
                        utime: p.utime,
                        stime: p.stime,
                        inicio: p.inicio,
                    },
                );
                // [por qué] Si el pid se reutilizó (otro `inicio`), no hay delta válido.
                if let Some(prev) = self.prev.get(&p.pid)
                    && prev.inicio == p.inicio
                    && djiffies > 0
                {
                    let dproc = (p.utime + p.stime).saturating_sub(prev.utime + prev.stime);
                    cpu += dproc as f64 / djiffies as f64 * f64::from(self.ncpu) * 100.0;
                }
                rss = rss.saturating_add(p.rss);
                io_r = io_r.saturating_add(p.io_r);
                io_w = io_w.saturating_add(p.io_w);
            }
            let pids: Vec<u32> = del_grupo.iter().map(|p| p.pid).collect();
            let puertos = puertos_de(&pids, &escuchas);
            out.push(Contenedor {
                id12: id12(id),
                nombre: id.clone(),
                sitio_uuid: None,
                dominio: None,
                imagen: comando_principal(&del_grupo),
                estado: EstadoContenedor {
                    estado: "running".into(),
                    salud: if puertos.is_empty() {
                        "sin-puerto".into()
                    } else {
                        "ok".into()
                    },
                    reinicios: 0,
                    desde: String::new(),
                },
                recursos: Recursos {
                    cpu_pct: cpu,
                    mem_usada: rss,
                    mem_limite: 0,
                    blk_ro: io_r,
                    blk_wo: io_w,
                    net_rx: 0,
                    net_tx: 0,
                },
                puertos,
            });
        }
        self.prev = nuevas;
        out
    }
}

/// Bucle del taller: un barrido cada `INTERVALO` (los mismos 10 s que la fuente Docker),
/// anota las transiciones y reemplaza el mapa entero (el barrido es completo: lo que no
/// está vivo desaparece, y con ello la fila pasa a `apagado` en el tablero).
pub async fn bucle(raiz: String, estado: std::sync::Arc<crate::samplers::EstadoCompartido>) {
    use std::collections::HashMap;
    use tokio::time::{Duration, interval};

    /// Cadencia del barrido: igual que `INTERVALO_STATS` de la fuente Docker.
    const INTERVALO: Duration = Duration::from_secs(10);
    let mut muestreador = MuestreadorProcesos::nuevo(raiz);
    let mut tick = interval(INTERVALO);
    loop {
        tick.tick().await;
        /* El IO del host no depende del origen de las filas: en modo `procesos` nadie más
         * llama al sampler de disco (lo hacía `ciclo_stats`, que es de Docker), y el panel
         * lo lee de `/snapshot`. */
        crate::samplers::ciclo_disco(&estado).await;
        let filas = muestreador.ciclo();
        crate::samplers::anotar_eventos(&estado, &filas).await;
        let mut mapa: HashMap<String, crate::samplers::Entrada> = HashMap::with_capacity(filas.len());
        for c in filas {
            mapa.insert(c.id12.clone(), crate::samplers::Entrada::nueva(c));
        }
        *estado.contenedores.write().await = mapa;
    }
}

/// Pies de las pruebas de los parsers, con formato real del kernel.
#[cfg(test)]
mod pruebas {
    use super::*;

    // `stat` real: el nombre lleva espacios y paréntesis a propósito.
    const STAT: &str = "1234 (bash -lc algo (raro)) S 1 1234 1234 0 -1 4194560 500 0 0 0 \
        12 34 5 6 20 0 3 0 987654 123456 78 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn stat_con_parentesis_en_el_nombre() {
        let (ppid, utime, stime, inicio) = parsear_stat(STAT).expect("stat válido");
        assert_eq!((ppid, utime, stime, inicio), (1, 12, 34, 987654));
    }

    #[test]
    fn stat_basura_no_revienta() {
        assert!(parsear_stat("basura").is_none());
        assert!(parsear_stat("1 (x) S 1 2").is_none());
    }

    #[test]
    fn status_da_rss_en_bytes_y_hilos() {
        let txt = "Name:\tnode\nVmRSS:\t  20480 kB\nThreads:\t7\n";
        assert_eq!(rss_de_status(txt), 20_971_520);
        assert_eq!(hilos_de_status(txt), 7);
        assert_eq!(rss_de_status("nada"), 0);
    }

    #[test]
    fn io_y_jiffies() {
        assert_eq!(io_de_proc("rchar: 9\nread_bytes: 4096\nwrite_bytes: 8192\n"), (4096, 8192));
        let stat = "cpu  100 0 50 1000 0 0 0 0 0 0\ncpu0 50 0 25 500 0 0 0 0 0 0\ncpu1 50 0 25 500 0 0 0 0 0 0\n";
        assert_eq!(jiffies_totales(stat), 1150);
        assert_eq!(ncpu_de_stat(stat), 2);
        // La línea agregada (`cpu `, con espacio) no es una CPU.
        assert_eq!(ncpu_de_stat("cpu  1 0 0 0 0 0 0 0 0 0\n"), 1);
    }

    #[test]
    fn direcciones_de_proc_net() {
        assert_eq!(ipv4_de_hex("0100007F").as_deref(), Some("127.0.0.1"));
        assert_eq!(ipv4_de_hex("00000000").as_deref(), Some("0.0.0.0"));
        assert!(ipv4_de_hex("0100").is_none());
        // ::1 en /proc/net/tcp6: 00000000000000000000000001000000.
        let v6 = ipv6_de_hex("00000000000000000000000001000000").expect("v6 válido");
        assert!(v6.ends_with(":0001"), "{v6}");
        assert!(ipv6_de_hex("00").is_none());
    }

    #[test]
    fn listen_solo_toma_escuchas() {
        let txt = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: 0100007F:0C26 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1001 0 8781244 2 0 100 0 0 10 0\n\
             1: 0100007F:1F90 00000000:0000 01 00000000:00000000 00:00000000 00000000  1001 0 999 2 0 100 0 0 10 0\n";
        let e = parsear_listen(txt, false);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].ip.as_str(), e[0].puerto, e[0].inodo), ("127.0.0.1", 3110, 8781244));
    }

    #[test]
    fn proyecto_solo_bajo_la_raiz() {
        let raiz = "/home/dev/taller";
        assert_eq!(id_proyecto("/home/dev/taller/NAKOMI", raiz).as_deref(), Some("NAKOMI"));
        assert_eq!(
            id_proyecto("/home/dev/taller/PROYECTO TASKS/frontend", raiz).as_deref(),
            Some("PROYECTO TASKS")
        );
        assert_eq!(id_proyecto("/home/dev/taller/_logs/NAKOMI", raiz), None);
        assert_eq!(id_proyecto("/home/dev/taller", raiz), None);
        assert_eq!(id_proyecto("/home/dev/bin", raiz), None);
        assert_eq!(id_proyecto("/home/dev/tallerX/a", raiz), None);
    }

    #[test]
    fn agrupa_semillas_y_arrastra_descendientes() {
        let raiz = "/home/dev/taller";
        let proc = |pid: u32, ppid: u32, cwd: &str| Proc {
            pid,
            ppid,
            cwd: cwd.to_owned(),
            cmdline: String::new(),
            utime: 0,
            stime: 0,
            inicio: 1,
            rss: 0,
            io_r: 0,
            io_w: 0,
        };
        let procs = vec![
            proc(10, 1, "/home/dev/taller/NAKOMI"),
            proc(11, 10, "/var/tmp/glory-target"), // hijo con otra cwd
            proc(12, 1, "/home/dev"),              // fuera del taller
            proc(13, 1, "/home/dev/taller/_logs/NAKOMI"),
        ];
        let g = agrupar(&procs, raiz);
        assert_eq!(g.len(), 1);
        assert_eq!(g["NAKOMI"], vec![0, 1]);
    }

    #[test]
    fn id12_es_estable_y_de_doce() {
        assert_eq!(id12("NAKOMI"), id12("NAKOMI"));
        assert_eq!(id12("NAKOMI").len(), 12);
        assert_ne!(id12("NAKOMI"), id12("PROYECTO TASKS"));
    }

    #[test]
    fn comando_principal_es_el_de_mas_memoria_y_se_recorta() {
        let mut a = Proc {
            pid: 1,
            ppid: 1,
            cwd: String::new(),
            cmdline: "bash -lc env".into(),
            utime: 0,
            stime: 0,
            inicio: 1,
            rss: 100,
            io_r: 0,
            io_w: 0,
        };
        let b = Proc {
            cmdline: "x".repeat(500),
            rss: 9999,
            ..a.clone()
        };
        a.rss = 100;
        assert_eq!(comando_principal(&[&a, &b]).len(), MAX_CMDLINE);
        assert_eq!(comando_principal(&[&a]), "bash -lc env");
    }
}
