# CHANGELOG (glory-pulse)

Formato: `## [x.y.z] - fecha` + cambios. El protocolo lleva su propia
versión (`schema` en el snapshot); aquí van releases del agente.

## [0.1.0] - 2026-09-29 (F1)

- Crate inicial: 3 muestreadores (eventos→sucio, stats 10s, meta Coolify 60s
  best-effort), `GET /snapshot` (Bearer) + `GET /health` (abierto).
- Snapshot en memoria, recorte determinista a 50, contrato `schema: 1`.
- Proxy `linuxserver/socket-proxy:version-3.4.4-r0` con allowlist mínima
  (`CONTAINERS/EVENTS/VERSION/PING` + `POST=0` + todo `ALLOW_*=0`).
- Token 32B hex obligatorio al arrancar; fuera de logs y respuestas.
