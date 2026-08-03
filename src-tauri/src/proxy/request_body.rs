use super::types::RequestContext;
use bytes::{Bytes, BytesMut};
use futures::{TryStream, TryStreamExt};
use http_body_util::BodyDataStream;
use hyper::body::Incoming;
use std::sync::{Arc, Mutex};

const REQUEST_OBSERVATION_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
struct ObservationState {
    bytes: BytesMut,
    overflowed: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RequestBodyObservation {
    state: Arc<Mutex<ObservationState>>,
}

impl RequestBodyObservation {
    fn observe(&self, chunk: &Bytes) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.overflowed {
            return;
        }
        if state.bytes.len().saturating_add(chunk.len()) > REQUEST_OBSERVATION_BYTES {
            state.bytes.clear();
            state.overflowed = true;
            return;
        }
        state.bytes.extend_from_slice(chunk);
    }

    pub(crate) fn apply_to_context(&self, context: &mut RequestContext) {
        let Ok(state) = self.state.lock() else {
            return;
        };
        if state.overflowed {
            return;
        }
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&state.bytes) else {
            return;
        };
        context.model = json
            .get("model")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| context.model.clone());
        context.stream = json
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(context.stream);
    }
}

pub(crate) enum ForwardRequestBody {
    Buffered(Bytes),
    Streaming {
        body: reqwest::Body,
        observation: Option<RequestBodyObservation>,
    },
}

impl ForwardRequestBody {
    pub(crate) fn observed_stream(body: Incoming) -> Self {
        let observation = RequestBodyObservation::default();
        Self::Streaming {
            body: observed_reqwest_body(BodyDataStream::new(body), &observation),
            observation: Some(observation),
        }
    }

    pub(crate) fn passthrough_stream(body: Incoming) -> Self {
        Self::Streaming {
            body: reqwest::Body::wrap_stream(BodyDataStream::new(body)),
            observation: None,
        }
    }

    pub(crate) fn into_parts(self) -> (reqwest::Body, Option<RequestBodyObservation>) {
        match self {
            Self::Buffered(bytes) => (bytes.into(), None),
            Self::Streaming { body, observation } => (body, observation),
        }
    }
}

fn observed_reqwest_body<S>(stream: S, observation: &RequestBodyObservation) -> reqwest::Body
where
    S: TryStream<Ok = Bytes> + Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let observer = observation.clone();
    reqwest::Body::wrap_stream(stream.map_ok(move |chunk| {
        observer.observe(&chunk);
        chunk
    }))
}

impl From<Bytes> for ForwardRequestBody {
    fn from(value: Bytes) -> Self {
        Self::Buffered(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use http_body_util::BodyExt;

    #[test]
    fn observation_extracts_small_json_metadata() {
        let observation = RequestBodyObservation::default();
        observation.observe(&Bytes::from_static(
            br#"{"model":"deepseek-chat","stream":true}"#,
        ));
        let mut context = RequestContext::default();
        observation.apply_to_context(&mut context);
        assert_eq!(context.model.as_deref(), Some("deepseek-chat"));
        assert!(context.stream);
    }

    #[test]
    fn observation_overflow_drops_metadata_without_rejecting_the_body() {
        let observation = RequestBodyObservation::default();
        observation.observe(&Bytes::from(vec![b'x'; REQUEST_OBSERVATION_BYTES + 1]));
        let mut context = RequestContext::default();
        observation.apply_to_context(&mut context);
        assert!(context.model.is_none());
        assert!(!context.stream);
    }

    #[tokio::test]
    async fn observed_stream_forwards_large_body_without_buffering_it() {
        let observation = RequestBodyObservation::default();
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; REQUEST_OBSERVATION_BYTES])),
            Ok(Bytes::from_static(b"tail")),
        ];
        let body = observed_reqwest_body(stream::iter(chunks), &observation);
        let forwarded = body.collect().await.unwrap().to_bytes();
        assert_eq!(forwarded.len(), REQUEST_OBSERVATION_BYTES + 4);

        let mut context = RequestContext::default();
        observation.apply_to_context(&mut context);
        assert!(context.model.is_none());
    }
}
