# glory-pulse
Agente minimo de solo lectura en la VPS: estado Docker/Coolify en tiempo real por HTTP (snapshot+poll). Plano de datos de la tab vps.

La fuente es conmutable por `PULSE_FUENTE` (`docker` en produccion, `procesos` en el taller de la 89).

Operacion: `docs/OPERACION.md` (runbook verificado) · compose prod: `deploy/docker-compose.prod.yaml` · skill: `glory-pulse-ops`.
