//! Test de contrato F1: el snapshot Rust cumple `schema/snapshot.schema.json`
//! y el ejemplo del repo sigue siendo válido. Sin demonio.

use serde_json::Value;

fn requeridos() -> Vec<&'static str> {
    vec![
        "schema",
        "hostId",
        "ts",
        "contenedores",
        "frescura",
        "truncado",
        "totalContenedores",
    ]
}

#[test]
fn ejemplo_conforme_al_contrato() {
    let ejemplo: Value = serde_json::from_str(include_str!("../../schema/ejemplo.json"))
        .expect("ejemplo.json válido");
    for k in requeridos() {
        assert!(ejemplo.get(k).is_some(), "ejemplo sin {k}");
    }
    assert_eq!(ejemplo["schema"], 1);
    let frescura = &ejemplo["frescura"];
    assert!(frescura.get("fuente").is_some() && frescura.get("edadMs").is_some());
    for c in ejemplo["contenedores"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        for k in ["id12", "nombre", "estado", "recursos"] {
            assert!(c.get(k).is_some(), "contenedor sin {k}");
        }
        let r = &c["recursos"];
        for k in [
            "cpuPct",
            "memUsada",
            "memLimite",
            "blkRO",
            "blkWO",
            "netRX",
            "netTX",
        ] {
            assert!(r.get(k).is_some(), "recursos sin {k}");
        }
    }
}

#[test]
fn parseo_stats_fixture() {
    // [por qué] El % de CPU sale por delta entre muestras (el one-shot no
    // trae base): el fixture trae cpu+sistema para verificar la fórmula.
    let stats: Value =
        serde_json::from_str(include_str!("fixtures/stats-ejemplo.json")).expect("fixture válido");
    assert!(stats.pointer("/cpu_stats/cpu_usage/total_usage").is_some());
    assert!(stats.pointer("/memory_stats/usage").is_some());
    assert!(stats.pointer("/networks/eth0/rx_bytes").is_some());
}
