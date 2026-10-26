# Roadmap — glory-pulse

**Contexto.** Agente Rust de solo lectura: cuenta los contenedores Docker de la VPS 89 y sirve el
snapshot por HTTP con Bearer (snapshot + poll), como plano de datos de la tab vps del tablero. Se
edita en el portátil; se compila y prueba en la 89, y producción es `https://pulse.wandori.us`
(Coolify, VPS 66, imagen `ghcr.io/1ndoryu/glory-pulse:sha-<short>`). Runbook: `docs/OPERACION.md`;
compose: `deploy/docker-compose.prod.yaml`; skill `glory-pulse-ops`.

## Siguiente bloque

- Ninguno.

## Pendientes

Ninguno ejecutable ahora: los dos pendientes restantes están bloqueados (ver abajo).

## Bloqueos

- Nota: **D2 (superficie pública o Tailscale), D3 (coste) y D4 (quién opera)**: decisiones de la usuaria,
  sin tomar (`docs/F0-spike.md:80`). No bloquean la operación actual, que va por Coolify.
- **Deploy y escrituras en producción**: reservado a la usuaria (AGENTS §3 del protocolo del área).
- **Tag del proxy sin re-verificar en el deploy.** Hecho externo, reservado a la usuaria: el próximo deploy en
  Coolify, reservado a la usuaria. Criterio: el compose aplicado en Coolify y
  `deploy/docker-compose.prod.yaml` coinciden en el tag `sha-<short>` fijado (si difieren, manda Coolify y hay
  que re-sincronizar el espejo). Antes `:latest` estaba prohibido; el tag del proxy no se re-comprobó desde el spike.
- **Reachability sin medir `workspace-manager → VPS 89 → pulse`.** Depende de D2 (superficie
  pública o Tailscale), decisión de la usuaria. Cuando D2 esté tomada: una consulta del tablero a `pulse` por el
  camino real, con el resultado anotado en `docs/OPERACION.md`.

## Planes activos
