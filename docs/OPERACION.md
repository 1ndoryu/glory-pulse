# Operación de glory-pulse (producción)

> Verificado contra vivo el 2026-09-30. Si un comando de aquí falla,
> el runbook está desactualizado: corrígelo antes de seguir.

## 1. Qué es y dónde vive

- **Servicio:** `pulse` en Coolify (`r4okw44w84c0ko88g844kosk`),
  URL pública `https://pulse.wandori.us`.
- **Qué hace:** cuenta **todos los contenedores Docker de la VPS**
  (no "sitios": cada stack tiene app+db+proxies, más Traefik e infra
  de Coolify; el 2026-09-30 eran 34) y los sirve por HTTP con Bearer.
- **Imagen:** `ghcr.io/1ndoryu/glory-pulse:<tag-fijo>` (tag `sha-<short>`
   del workflow `docker.yml`; `:latest` prohibido). Vivo: `sha-6479252`
   (= `glory-pulse@6479252`, 2026-10-06: 0110A-1 `discoHost` + 309A-2
   `/detalle`; deploy con aviso ajeno `nakomi: error` preexistente).
- **Compose canónico:** `deploy/docker-compose.prod.yaml` (espejo del
  aplicado en Coolify; si difieren, manda Coolify y hay que re-sincronizar).

## 2. Comprobación rápida (smoke test)

```powershell
# 1. Liveness pública (sin auth; 200 + contenedores=N)
Invoke-RestMethod https://pulse.wandori.us/health
# {"estado":"ok","schema":1,"contenedores":34,...}

# 2. http redirige a https (302)
Invoke-WebRequest http://pulse.wandori.us -MaximumRedirection 0

# 3. /snapshot exige Bearer (401 sin token; 200 con token)
Invoke-WebRequest https://pulse.wandori.us/snapshot
```

El token vive **solo** en Coolify (env `PULSE_TOKEN` del sitio) y en el
consumidor (`workspace-manager` backend: `PULSE_TOKEN`). **Nunca** en el
repo, en logs ni en este runbook. En logs solo aparece `token_len=64`.

## 3. Operar con el manager

Binario fuente (único con `wordpress`+rust-image), config autoritativa:

```powershell
$cm = 'C:\tmp\glory-target\coolify-manager\debug\coolify-manager.exe'
$cfg = 'C:\Users\Owner\OneDrive\Documentos\area-trabajo\coolify-manager-rs\config\settings.json'
& $cm -c $cfg health -n pulse        # probe externo: up + ms + code
& $cm -c $cfg deploy-service -n pulse --skip-backup  # redeploy Rust (NO restart --all)
& $cm -c $cfg logs -n pulse --target app --lines 50
```

Límites conocidos (2026-09-30, no reintentar a ciegas):
- `diagnose` **no** lista contenedores de stacks Coolify (salida vacía).
- `exec` es **intra-contenedor** (no hay host-exec ni `docker kill`).
- `container-events -n pulse` **se cuelga** (sin salida en 3-5 min).
- `redeploy -n pulse` **rechaza** el molde rust-image (`falta 'REPO_URL:'`).
- Latencia VPS↔oficina: suelo ~100 ms RTT; `/snapshot` p50 ≈107 ms con
  keep-alive (~433 ms sin él); el servidor aporta ≈2 ms (cache SWR).
  Un p99 e2e <50 ms es físicamente imposible desde aquí.

## 4. Actualizar la imagen (deploy normal)

1. Merge a `main` en `1ndoryu/glory-pulse` → GHA publica `sha-<short>`.
2. Cambiar `image:` en `deploy/docker-compose.prod.yaml` al tag nuevo.
3. Aplicar: `set-compose -n pulse --compose-file deploy/docker-compose.prod.yaml`
   (valida ASCII/256KiB/`services:`/sin `build:` y verifica con GET).
4. `deploy-service -n pulse --skip-backup` y smoke test (§2).
5. Si el health no vuelve a `ok` en ~3 min: el deploy restaura el compose
   anterior solo (rollback); revisar `logs --target app`.

## 5. Lecciones que no se repiten (F4)

- **El sidecar `socket-proxy` es obligatorio.** Sin él, pulse hace
  `exit(1)` "sin Docker" → `degraded:unhealthy` → rollback en cadena.
  `DOCKER_HOST=http://socket-proxy:2375`; pulse **jamás** monta
  `docker.sock` (solo el proxy lo monta `:ro`).
- **Sin `healthcheck` en `app`.** La imagen (debian-slim, `USER 65534`,
  sin curl) no trae curl; el gate es el probe externo + `GET /health`.
- **Traefik `Host()` siempre con backticks** (234B): sin ellos el dominio
  se interpreta como template Go → sin certificado.
- **Nunca compilar en la VPS** (Rev.4): el build del 2026-09-20 reinició
  `dockerd`. Patrón `rust-image`: GHA→GHCR + pull de tag fijo.
- **Nunca `restart --all`** con workloads Rust (los deja en `exited`).
- Proxy pineado `lscr.io/linuxserver/socket-proxy:version-3.4.4-r0`
  (tecnativa ≥0.5.1 por CVE-2026-78122: `ALLOW_*=0` por endpoint).

## 6. Consumidor workspace-manager

Backend `GET /api/vps/agente` (5 s SWR, breaker: 3 fallos→30 s,
1 éxito→cierra). Sin `PULSE_URL`+`PULSE_TOKEN` (≥32 chars) responde
`sin-configurar` y la UI usa el legacy. Contrato de filas = F0 anidado
(`schema/ejemplo.json`); el frontend muestra insignia `agente` +
`en vivo · N contenedores · hace Xs` y pausa el poll con tab oculta.
Detalle: 309A-2 (el drill-down legacy son 7 piezas SSH en serie, ~120 s).
