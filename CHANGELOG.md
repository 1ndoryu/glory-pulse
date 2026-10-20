# CHANGELOG (glory-pulse)

Formato: `## [x.y.z] - fecha` + cambios. El protocolo lleva su propia
versión (`schema` en el snapshot); aquí van releases del agente.

## [0.4.0] - 2026-10-20 (19AA-3)

- Fuente conmutable por env `PULSE_FUENTE`: `docker` (defecto) o `procesos`.
  En modo `procesos` no se conecta al API Docker.
- En modo `procesos` cada "contenedor" del snapshot es un proyecto del taller
  leído de `/proc` (semilla por `cwd` bajo `PULSE_RAIZ_TALLER`, por defecto
  `/home/dev/taller`; descendientes por PPID; CPU % por delta de jiffies;
  puertos de los LISTEN de `/proc/net/tcp{,6}` cruzados por inodo con
  `/proc/<pid>/fd`; `id12` = hash FNV-1a; `imagen` = cmdline del proceso de
  más RSS recortada a 120 caracteres). Sigue prohibido exec/spawn.
- `GET /pieza?sitio=<proyecto>&nombre=<pieza>&lineas=<n>` (Bearer) en modo
  `procesos`, con las 7 piezas `salud|stats|inspeccion|eventos|bd|diagnostico|
  logs`; fuera de ese modo responde 404 `piezas-solo-en-procesos`. `sitio` es
  el nombre del proyecto del taller (admite espacios), fail-closed.
- `/health` informa además del campo `fuente`.
- `schema` sigue en 1 (los campos nuevos son aditivos).
- Uso real: `Arranque up glory-pulse` en la VPS 89, sin Docker ni sudo.

## [0.3.0] - 2026-10-06 (0110A-1, F1+F3; imagen `sha-6479252` en prod)

- Campo `discoHost` top-level en el snapshot (`sectoresLeidos` /
  `sectoresEscritos` acumulados de `/proc/diskstats`, solo discos físicos
  enteros; particiones y `loop/ram/dm/md` excluidos). Ceros donde `/proc`
  no se lee (`schema` sigue en 1, compatible hacia atrás).
- Fuente anulable por env `DISKSTATS_PATH` (defecto `/proc/diskstats`).

## [0.2.0] - 2026-10-05 (309A-2)

- `GET /detalle?sitio=<uuid>` (Bearer): contenedores del sitio en una sola
  conexión, solo memoria (sin tocar Docker). `sitio` fail-closed; sitio
  desconocido = 200 con lista vacía.
- Campo `imagen` en contenedor (del `list`, gratis); se omite si vacío
  (`schema` sigue en 1, compatible hacia atrás).
- Regla de pertenencia: nombre `<pref>-<uuid>` (app/postgres/socket-proxy/
  mariadb/wordpress) o `sitioUuid` de la meta Coolify.

## [0.1.0] - 2026-09-29 (F1)

- Crate inicial: 3 muestreadores (eventos→sucio, stats 10s, meta Coolify 60s
  best-effort), `GET /snapshot` (Bearer) + `GET /health` (abierto).
- Snapshot en memoria, recorte determinista a 50, contrato `schema: 1`.
- Proxy `linuxserver/socket-proxy:version-3.4.4-r0` con allowlist mínima
  (`CONTAINERS/EVENTS/VERSION/PING` + `POST=0` + todo `ALLOW_*=0`).
- Token 32B hex obligatorio al arrancar; fuera de logs y respuestas.
