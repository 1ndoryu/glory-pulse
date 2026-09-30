# glory-pulse — notas de agente

Agente minimo de solo lectura en la VPS: snapshot Docker por HTTP con Bearer.
Plano de datos de la tab `vps` de workspace-manager.

## Fuentes canonicas (leer antes de operar)

- `docs/OPERACION.md` — runbook verificado contra vivo (smoke test, deploy,
  limites del manager, lecciones F4). Manda sobre cualquier otro doc.
- `deploy/docker-compose.prod.yaml` — compose canonico aplicado en Coolify.
  Si difiere de Coolify, re-sincronizar en el acto.
- `schema/ejemplo.json` — contrato F0 de filas (anidadas) que consume
  `workspace-manager/src/server/vps/agente.ts`.
- Skill global `glory-pulse-ops` — atajo operativo (ver `docs/OPERACION.md` §1).

## Decisiones vigentes (2026-09-30)

- D2: Bearer permanente aceptado (rotacion manual si se expone).
- D3: mantener pulse (el legacy queda como fallback tras breaker).
- D4: opera el assistant (push y redeploys; `BORRAR` sigue siendo del usuario).

## Reglas duras

- Jamas compilar en la VPS (GHA→GHCR + pull de tag fijo `sha-<short>`).
- Sidecar `socket-proxy` obligatorio; pulse jamas monta `docker.sock`.
- Sin `healthcheck` en `app` (imagen sin curl); gate = probe + `GET /health`.
- Traefik `Host()` con backticks; `restart --all` prohibido con Rust.
- Token solo en Coolify/env; en logs unicamente `token_len=64`.

## Pendiente

- 309A-2 (propuesto): endpoint de detalle por sitio en una sola conexion
  (el drill-down legacy son 7 piezas SSH en serie, ~120 s medidos).
