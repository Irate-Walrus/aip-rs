//! The AIP-193 error-domain boundary layer.
//!
//! AIP-193's `ErrorInfo.domain` is "typically the registered service name", and
//! a service must present exactly one domain across its whole error surface. The
//! aip-rs library cannot know the deploying service, so every error it maps
//! stamps the library-internal sentinel `aip-rs` (ADR-0007). That sentinel means
//! "replace me at the boundary" — it must never reach a client.
//!
//! This crate is that boundary. [`Layer::new`] takes the service's domain and
//! wraps a tonic server so that, on the way out, the `aip-rs` sentinel in
//! `grpc-status-details-bin` is rewritten to the service's domain — once, at the
//! serving edge, instead of re-stamped at every call site. A service-raised or
//! third-party domain is never touched; only the exact sentinel
//! [`SENTINEL_DOMAIN`] is rewritten.
//!
//! ```
//! # use tonic::transport::Server;
//! const SERVICE_DOMAIN: &str = "freight.example.com";
//! // Install once on the server builder, ahead of `.add_service(…)`:
//! let _builder = Server::builder().layer(aip_errordomain::Layer::new(SERVICE_DOMAIN));
//! ```
//!
//! This crate carries no aip-rs `Error` type — it rewrites `google.rpc.Status`
//! bytes at the transport edge, so it is not the shared error crate ADR-0001
//! rejected. See `docs/adr/0007-aip193-error-details.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use base64::alphabet;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine as _;
use bytes::Bytes;
use http::header::HeaderName;
use http::{HeaderMap, HeaderValue};
use pin_project_lite::pin_project;
use prost::Message as _;
use tonic_types::pb;

/// The library-internal sentinel `ErrorInfo.domain` that aip-rs stamps when it
/// has no service domain of its own (ADR-0007). It means "replace at the
/// boundary"; the [`Layer`] rewrites exactly this value and nothing else.
pub const SENTINEL_DOMAIN: &str = "aip-rs";

/// The gRPC trailer carrying the binary `google.rpc.Status` (AIP-193's rich
/// error details). It rides in the response *headers* on a trailers-only unary
/// error and in the real *trailers* on a streaming error, so the layer rewrites
/// both.
const GRPC_STATUS_DETAILS_BIN: HeaderName = HeaderName::from_static("grpc-status-details-bin");

/// The fully-qualified type name of the detail whose `domain` is rewritten. A
/// `google.rpc.Status` packs its details as `Any`s keyed by a `type.googleapis.com/…`
/// URL; only the `ErrorInfo` detail carries a domain.
const ERROR_INFO_TYPE: &str = "google.rpc.ErrorInfo";

/// Decode `grpc-status-details-bin` with the standard base64 alphabet, accepting
/// padded or unpadded input. tonic encodes the header without padding but its
/// own decoder is padding-indifferent; matching that keeps the layer robust to
/// either spelling.
const DECODE: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// A tonic/tower [`Layer`](tower::Layer) that rewrites the [`SENTINEL_DOMAIN`] in
/// outgoing AIP-193 error details to the deploying service's own domain, so the
/// service presents one `ErrorInfo.domain` to its clients (ADR-0007).
///
/// Install it once on the server builder; see the [crate docs](crate) for an
/// example.
#[derive(Clone, Debug)]
pub struct Layer {
    domain: Arc<str>,
}

impl Layer {
    /// Builds the layer for `domain` — the service's AIP-193 domain, typically
    /// its registered service name (e.g. `freight.example.com`).
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: Arc::from(domain.into()),
        }
    }
}

impl<S> tower::Layer<S> for Layer {
    type Service = DomainService<S>;

    fn layer(&self, inner: S) -> DomainService<S> {
        DomainService {
            inner,
            domain: self.domain.clone(),
        }
    }
}

/// The [`Service`](tower::Service) [`Layer`] wraps the inner stack in. It passes
/// the request through untouched and rewrites the sentinel domain on the way out
/// — in the response headers (trailers-only unary errors) and, by wrapping the
/// body, in the real trailers (streaming errors).
#[derive(Clone, Debug)]
pub struct DomainService<S> {
    inner: S,
    domain: Arc<str>,
}

impl<S, ReqBody, ResBody> tower::Service<http::Request<ReqBody>> for DomainService<S>
where
    S: tower::Service<http::Request<ReqBody>, Response = http::Response<ResBody>>,
{
    type Response = http::Response<RewriteBody<ResBody>>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        ResponseFuture {
            inner: self.inner.call(req),
            domain: self.domain.clone(),
        }
    }
}

pin_project! {
    /// The future returned by [`DomainService`]: it rewrites the response headers
    /// and wraps the body once the inner future resolves.
    pub struct ResponseFuture<F> {
        #[pin]
        inner: F,
        domain: Arc<str>,
    }
}

impl<F, ResBody, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<http::Response<ResBody>, E>>,
{
    type Output = Result<http::Response<RewriteBody<ResBody>>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let response = ready!(this.inner.poll(cx))?;
        let (mut parts, body) = response.into_parts();
        // A trailers-only response (the common unary error) carries the status
        // details in the headers, so rewrite them here before the body's
        // trailers (which won't exist) are ever polled.
        rewrite_details(&mut parts.headers, this.domain);
        let body = RewriteBody {
            inner: body,
            domain: this.domain.clone(),
        };
        Poll::Ready(Ok(http::Response::from_parts(parts, body)))
    }
}

pin_project! {
    /// A response body that rewrites the sentinel domain in the `grpc-status-details-bin`
    /// trailer as it streams past, leaving every data frame untouched. Wrapping
    /// the body is how a streaming error — whose status rides in real trailers,
    /// not the headers — gets the same single-domain treatment as a unary error.
    pub struct RewriteBody<B> {
        #[pin]
        inner: B,
        domain: Arc<str>,
    }
}

impl<B> http_body::Body for RewriteBody<B>
where
    B: http_body::Body,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                // Only a trailers frame carries the status details; data frames
                // pass straight through.
                let frame = match frame.into_trailers() {
                    Ok(mut trailers) => {
                        rewrite_details(&mut trailers, this.domain);
                        http_body::Frame::trailers(trailers)
                    }
                    Err(data) => data,
                };
                Poll::Ready(Some(Ok(frame)))
            }
            other => Poll::Ready(other),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// Rewrites the [`SENTINEL_DOMAIN`] to `domain` in a `grpc-status-details-bin`
/// header/trailer, in place. A no-op when the header is absent, undecodable, or
/// carries no sentinel `ErrorInfo` — the boundary never breaks a response it
/// can't rewrite, and a service-raised or third-party domain passes untouched.
fn rewrite_details(headers: &mut HeaderMap, domain: &str) {
    let Some(current) = headers.get(&GRPC_STATUS_DETAILS_BIN) else {
        return;
    };
    let Some(rewritten) = rewrite_status_bytes(current.as_bytes(), domain) else {
        return;
    };
    if let Ok(value) = HeaderValue::from_maybe_shared(rewritten) {
        headers.insert(GRPC_STATUS_DETAILS_BIN, value);
    }
}

/// The pure rewrite rule over a base64-encoded `google.rpc.Status`: returns the
/// re-encoded header value *only* when an `ErrorInfo.domain` equal to the
/// sentinel was found and replaced, so a caller leaves the original bytes in
/// place otherwise. Any decode failure yields `None` — an unparseable status is
/// passed through verbatim rather than dropped.
fn rewrite_status_bytes(b64: &[u8], domain: &str) -> Option<Bytes> {
    let raw = DECODE.decode(b64).ok()?;
    let mut status = pb::Status::decode(raw.as_slice()).ok()?;

    let mut rewrote = false;
    for any in &mut status.details {
        if !is_error_info(&any.type_url) {
            continue;
        }
        let Ok(mut info) = pb::ErrorInfo::decode(any.value.as_slice()) else {
            // A detail typed as ErrorInfo but undecodable: leave its bytes be.
            continue;
        };
        // Sentinel-only: a service-raised or third-party domain is left alone.
        if info.domain == SENTINEL_DOMAIN {
            info.domain = domain.to_owned();
            any.value = info.encode_to_vec();
            rewrote = true;
        }
    }

    if !rewrote {
        return None;
    }
    // Re-encode without padding, matching how tonic spells the header on the
    // wire, so the rewritten value is byte-compatible with what a client reads.
    Some(Bytes::from(STANDARD_NO_PAD.encode(status.encode_to_vec())))
}

/// Whether an `Any` type URL names `google.rpc.ErrorInfo`, tolerant of the
/// `type.googleapis.com/` host prefix (or its absence).
fn is_error_info(type_url: &str) -> bool {
    type_url.rsplit('/').next().unwrap_or(type_url) == ERROR_INFO_TYPE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tonic::Code;
    use tonic_types::{ErrorDetails, StatusExt};

    /// Serialize a `tonic::Status` to the header map tonic would put on the wire,
    /// so the rewrite rule is tested against the exact `grpc-status-details-bin`
    /// bytes tonic emits.
    fn header_map(status: &tonic::Status) -> HeaderMap {
        let mut map = HeaderMap::new();
        status
            .add_header(&mut map)
            .expect("status serializes to a header map");
        map
    }

    /// Read the `ErrorInfo` back out of a header map the way a tonic client does.
    fn error_info(map: &HeaderMap) -> tonic_types::ErrorInfo {
        tonic::Status::from_header_map(map)
            .expect("a status is present")
            .get_error_details()
            .error_info()
            .expect("an ErrorInfo is attached")
            .clone()
    }

    fn sentinel_status() -> tonic::Status {
        let mut details = ErrorDetails::new();
        details.set_error_info(
            "FIELD_REQUIRED",
            SENTINEL_DOMAIN,
            HashMap::from([("fields".to_owned(), "display_name".to_owned())]),
        );
        details.add_bad_request_violation("site.display_name", "field is required");
        tonic::Status::with_error_details(Code::InvalidArgument, "missing field", details)
    }

    #[test]
    fn rewrites_the_sentinel_domain() {
        let mut map = header_map(&sentinel_status());
        rewrite_details(&mut map, "freight.example.com");

        let info = error_info(&map);
        assert_eq!(info.domain, "freight.example.com");
        // The reason and metadata ride through unchanged — only the domain moves.
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(
            info.metadata.get("fields").map(String::as_str),
            Some("display_name")
        );
    }

    #[test]
    fn preserves_other_details_when_rewriting() {
        let mut map = header_map(&sentinel_status());
        rewrite_details(&mut map, "freight.example.com");

        // The co-resident BadRequest survives the round-trip — the rewrite
        // touches only the ErrorInfo.domain, not the rest of the status.
        let status = tonic::Status::from_header_map(&map).expect("status present");
        let bad = status
            .get_details_bad_request()
            .expect("BadRequest still attached");
        assert_eq!(bad.field_violations[0].field, "site.display_name");
    }

    #[test]
    fn leaves_a_service_domain_untouched() {
        // A status already raised under the service's own domain (a `Validator`
        // error) is not the sentinel, so the layer must not touch it.
        let mut details = ErrorDetails::new();
        details.set_error_info(
            "RESOURCE_NAME_PATTERN_MISMATCH",
            "freight.example.com",
            HashMap::new(),
        );
        let status = tonic::Status::with_error_details(Code::InvalidArgument, "bad name", details);
        let before = header_map(&status);
        let mut after = before.clone();

        rewrite_details(&mut after, "other.example.com");

        assert_eq!(
            before[&GRPC_STATUS_DETAILS_BIN], after[&GRPC_STATUS_DETAILS_BIN],
            "a non-sentinel domain is left byte-identical"
        );
        assert_eq!(error_info(&after).domain, "freight.example.com");
    }

    #[test]
    fn leaves_a_third_party_domain_untouched() {
        let mut details = ErrorDetails::new();
        details.set_error_info("QUOTA_EXCEEDED", "compute.googleapis.com", HashMap::new());
        let status = tonic::Status::with_error_details(Code::ResourceExhausted, "quota", details);
        let mut map = header_map(&status);

        rewrite_details(&mut map, "freight.example.com");

        assert_eq!(error_info(&map).domain, "compute.googleapis.com");
    }

    #[test]
    fn is_a_no_op_without_status_details() {
        // A plain status (no rich details) has no `grpc-status-details-bin`, so
        // there is nothing to rewrite and the header map is left as-is.
        let status = tonic::Status::new(Code::Internal, "boom");
        let mut map = header_map(&status);
        let before = map.clone();

        rewrite_details(&mut map, "freight.example.com");

        assert_eq!(map.get(&GRPC_STATUS_DETAILS_BIN), None);
        assert_eq!(map, before);
    }

    #[test]
    fn is_a_no_op_on_undecodable_details() {
        // Garbage in the header is passed through, not dropped — the boundary
        // never breaks a response it cannot parse.
        let mut map = HeaderMap::new();
        map.insert(
            GRPC_STATUS_DETAILS_BIN,
            HeaderValue::from_static("not-valid-base64-$$$"),
        );
        let before = map.clone();

        rewrite_details(&mut map, "freight.example.com");

        assert_eq!(map, before);
    }

    #[test]
    fn matches_error_info_type_url_spellings() {
        assert!(is_error_info("type.googleapis.com/google.rpc.ErrorInfo"));
        assert!(is_error_info("google.rpc.ErrorInfo"));
        assert!(!is_error_info("type.googleapis.com/google.rpc.BadRequest"));
    }
}
