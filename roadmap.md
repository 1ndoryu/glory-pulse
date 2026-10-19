# Roadmap — glory-pulse

**Contexto.** Agente Rust de solo lectura: cuenta los contenedores Docker de la VPS 89 y sirve el
snapshot por HTTP con Bearer (snapshot + poll), como plano de datos de la tab vps del tablero. Se
edita en el portátil; se compila y prueba en la 89, y producción es `https://pulse.wandori.us`
(Coolify, VPS 66, imagen `ghcr.io/1ndoryu/glory-pulse:sha-<short>`). Runbook: `docs/OPERACION.md`;
compose: `deploy/docker-compose.prod.yaml`; skill `glory-pulse-ops`.

## Siguiente bloque

- [ ] `19AA-1` — **Re-verificar el tag exacto del proxy en el próximo deploy.** El runbook prohíbe
  `:latest` y fija el tag `sha-<short>` del workflow `docker.yml`, pero el tag del proxy no se
  re-comprobó desde el spike. Criterio: el compose aplicado en Coolify y
  `deploy/docker-compose.prod.yaml` coinciden en el tag fijado (si difieren, manda Coolify y hay que
  re-sincronizar el espejo).

## Pendientes

- [ ] `19AA-2` — **Medir la reachability real `workspace-manager → VPS 89 → pulse`.** No es medible
  hasta que el frente F4 del spike esté cerrado; hoy solo se ha probado desde fuera
  (`docs/F0-spike.md:80`). Criterio: una consulta del tablero a `pulse` por el camino real, con el
  resultado anotado en `docs/OPERACION.md`.

## Bloqueos

- **D2 (superficie pública o Tailscale), D3 (coste) y D4 (quién opera)**: decisiones de la usuaria,
  sin tomar (`docs/F0-spike.md:80`). No bloquean la operación actual, que va por Coolify.
- **Deploy y escrituras en producción**: reservado a la usuaria (AGENTS §3 del protocolo del área).

## Planes activos
