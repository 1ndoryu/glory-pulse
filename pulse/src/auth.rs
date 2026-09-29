//! Guardia Bearer para `/snapshot`. `/health` queda abierto (liveness de Coolify).
//!
//! [por qué] Comparación en tiempo constante para no filtrar el token por
//! timing. El token jamás se registra (solo su longitud al arrancar).

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone)]
pub struct Guardia {
    token: String,
}

impl Guardia {
    pub fn nuevo(token: String) -> Self {
        Self { token }
    }

    pub fn longitud(&self) -> usize {
        self.token.len()
    }
}

fn igual(a: &str, b: &str) -> bool {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    if x.len() != y.len() || x.is_empty() {
        return false;
    }
    x.iter().zip(y).fold(0u8, |acc, (p, q)| acc | (p ^ q)) == 0
}

pub async fn bearer(
    guardia: axum::extract::State<Guardia>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| igual(t, &guardia.token))
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
