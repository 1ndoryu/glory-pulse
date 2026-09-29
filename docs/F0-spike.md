# F0 — spike previo (2026-09-29, ejecutado)

Puerta de evidencia antes de F1. Veredicto global: **SÍ condicionado**
(los 5 puntos responden sí, con condiciones registradas abajo).
Fuentes: binario `C:\Users\Owner\bin\coolify-manager.exe`
(`coolify-manager 1.0.0`), Coolify `4.0.0-beta.460`, VPS `66.94.100.241`.

## V1 — Montar el socket es posible: SÍ condicionado

- `new --help` no expone flags de mounts: el alta de servicios es por
  compose sincronizado con la API (`deploy-service` sincroniza compose).
- `deploy-service --help`: despliega un sitio de `settings.json` con
  `--skip-compose-sync` / `--skip-backup`; el compose admite `volumes:`
  estándar (verificado en `diagnose→compose.services.app.volumes`).
- El socket NO lo monta pulse directo: lo monta el **proxy** (`proxy.sock:ro`
  en pulse, `/var/run/docker.sock:ro` solo en el proxy). Patrón estándar,
  sin evidencia en contra.
- Condición: la prueba final es el propio deploy (F4). Si Coolify rechaza
  el bind del socket, fallback: pulse sin socket + solo meta-Coolify.

## V2 — Labels disponibles: NO (vía manager) → meta por API con token read

- `diagnose -n glory-rest --json` OK (`10848 bytes`): `containers` es tabla
  de texto (`"NAMES STATUS IMAGE"`), `compose` trae build/volumes/env —
  **sin labels Docker**.
- `container-inspect -n glory-rest --json` → `ERROR Validacion: Salida de
  docker inspect inesperada para '05144c3e3ba7'` (bug manager, piezas
  insuficientes; causa probable: `mb_detect_encoding` / parseo, misma
  familia que el 422 de 268A-5).
- Decisión F1: meta-coolify (`sitioUuid`, dominio) vía **token Coolify con
  permiso `read`** (los secretos van redactados por defecto; `read:sensitive`
  NO necesario), cache 60 s. Sin fix del manager.

## V3 — Baseline legacy (medido, `n` pequeño, declarar varianza)

| Medición | Valor |
|---|---|
| `GET /api/vps/sitios` (WM, warm, `n=8`) | **1.3 s** (antes ~20 s en frío; varianza SSH no caracterizada) |
| `GET /api/vps/detalle?sitio=glory-rest` (WM) | **119.5 s** (`salud=ok stats=ok inspeccion=FALLO eventos=FALLO bd=ok diagnostico=ok logs=ok`) |
| `coolify-manager.exe list` directo | **0.1 s** por llamada |
| `list` ×3 en paralelo (3 procesos) | **0.1 s** total → el enlace VPS tolera concurrencia; sin degradación |
| `diagnose --json` | `10848 bytes` (orden de magnitud del snapshot futuro) |

Implicación D3: el coste real está en `detalle` (serie + 2 piezas
fallando), no en `sitios` warm. El objetivo p99 `<500 ms` sigue en pie
contra `detalle` tibio.

## V4 — Proxy y allowlist: SÍ, con pin y CVE registrado

- Proxy recomendado: **`linuxserver/socket-proxy:version-3.4.4-r0` pineado**
  (granularidad por endpoint vía `ALLOW_*`; alternativa:
  `tecnativa/docker-socket-proxy`).
- **CVE-2026-78122** (2026-08-22, CVSS 7.4): `tecnativa` `<0.5.1` con
  `CONTAINERS=1` expone `archive/export/logs/top` (lectura arbitraria).
  Exigir `≥0.5.1` si se usa tecnativa; con linuxserver, allowlist explícita
  mínima (`GET /containers/json`, `GET /containers/{id}/json`,
  `GET /containers/{id}/stats?stream=false`, `GET /version`, `GET /_ping`;
  `POST=0`, `EXEC=0`) + test F4 de denegación.
- Allowlist Coolify: existe **IP allowlist solo para la API de Coolify**
  (`Settings > Configuration > Advanced`, instance-wide); NO hay allowlist
  por app en la UI. Opciones para D2: (a) middleware Traefik `ipAllowList`
  manual sobre el router de pulse (frágil ante upgrades); (b) **Tailscale**
  — el manager trae comando `tailscale` nativo ("Prepara Tailscale en el
  host VPS"), plan B viable sin código extra.

## V5 — Semáforo: verde medido, decisión para F2

- Paralelo ×3 sin degradación ⇒ subir el semáforo a 2–3 es seguro para el
  enlace. Decisión: F2 sustituye la cola única por **semáforo por familia**
  (`list`-class en paralelo, `inspect`-class serie por sitio), con medición
  en ruta antes/después. El poller de pulse (F1) queda serie (un solo
  emisor, sin contención posible).

## Salidas F0 (en este repo)

- `schema/snapshot.schema.json` — contrato del snapshot.
- `schema/ejemplo.json` — ejemplo sintético conforme al esquema.
- Nota de viabilidad: este archivo (sí condicionado).

## Pendiente que F0 NO cierra (para F1/F4 o D2)

- Tag exacto del proxy a la hora del deploy (re-verificar, `:latest` prohibido).
- Reachability real `WM→VPS→pulse` (solo medible tras F4).
- D2 (superficie pública vs Tailscale), D3 (coste), D4 (quién opera).
