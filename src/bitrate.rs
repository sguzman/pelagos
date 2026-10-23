//! Authenticated Jellyfin v12.3 network bitrate test.
//! Random bytes are generated in bounded chunks; no whole-response allocation.
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::Deserialize;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::io::ReaderStream;

use crate::{app::AppState, auth_api};

const DEFAULT_BYTES: usize = 102_400;
const MAX_BYTES: usize = 100_000_000;
const CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug, Default, Deserialize)]
struct BitrateQuery {
    size: Option<i64>,
}

struct RandomReader {
    remaining: usize,
}

impl AsyncRead for RandomReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.remaining == 0 || output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let n = self.remaining.min(output.remaining()).min(CHUNK_BYTES);
        let mut chunk = [0_u8; CHUNK_BYTES];
        if let Err(error) = getrandom::fill(&mut chunk[..n]) {
            return Poll::Ready(Err(io::Error::other(format!(
                "random bitrate buffer unavailable: {error}"
            ))));
        }
        output.put_slice(&chunk[..n]);
        self.remaining -= n;
        Poll::Ready(Ok(()))
    }
}

async fn bitrate_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BitrateQuery>,
) -> Result<Response, StatusCode> {
    auth_api::authenticated(state, headers).await?;
    let requested = query.size.unwrap_or(DEFAULT_BYTES as i64);
    let size = usize::try_from(requested).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !(1..=MAX_BYTES).contains(&size) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let body = Body::from_stream(ReaderStream::new(RandomReader { remaining: size }));
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(response)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/Playback/BitrateTest", get(bitrate_test))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn random_stream_produces_exact_length_without_preallocating_entire_body() {
        let mut read = RandomReader { remaining: 100_123 };
        let mut content = Vec::new();
        read.read_to_end(&mut content).await.unwrap();
        assert_eq!(content.len(), 100_123);
        assert!(content.iter().any(|byte| *byte != 0));
        assert!(content.windows(2).any(|pair| pair[0] != pair[1]));
        assert_eq!(read.remaining, 0);
    }
}
