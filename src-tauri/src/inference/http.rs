//! Hardened cloud HTTP control transport and read-only client for `/mc/v1` wire protocol.
//!
//! Provides an immutable, security-hardened HTTP transport layer for querying health,
//! inspecting advertised model capabilities, checking job execution status, and safely
//! downloading bounded PNG inference results.
//!
//! ## Invariants
//!
//! 1. **Immutable Validated Target:** Constructed exclusively from validated profiles via
//!    [`crate::inference::config::validate_https_endpoint`]. Scheme MUST be `https://`, host
//!    must be a valid domain name, and query strings/fragments/credentials are forbidden.
//! 2. **Process-Global Single-Worker DNS & Anti-SSRF:** Uses a single process-global background
//!    worker with a bounded queue (`sync_channel(32)`). Resolves hostnames with explicit deadline,
//!    rejects ANY non-public/reserved IP (including private, loopback, link-local, CGNAT,
//!    documentation, and non-global IPv6 ranges), and pins validated addresses to the client
//!    connection. If the DNS worker queue is saturated, new requests fail closed immediately.
//! 3. **Strict IPv6 Allowlist:** Allows ONLY global unicast within `2000::/3` while explicitly
//!    excluding all known special and transition prefixes (`2001::/23`, `2002::/16`, `3ffe::/16`, `3fff::/16`).
//! 4. **Transport Hardening & No Retries:** Default rustls TLS certificate verification, redirects
//!    are strictly denied (`Policy::none()`), environment proxies are ignored (`no_proxy()`),
//!    automatic retries are disabled, and connect and request timeouts are strictly bounded.
//! 5. **Typed Runtime Credential Binding:** Credentials must be typed [`RuntimeCredential`] variants
//!    ([`RuntimeCredential::BeamBearer`] for Beam, [`RuntimeCredential::ModalProxy`] with `Modal-Key`
//!    and `Modal-Secret` headers for Modal) immutably bound to the target provider, profile ID,
//!    and canonical origin fingerprint via [`BoundRuntimeCredential`].
//! 6. **Static Sanitized Error Reporting:** Public errors never echo untrusted MIME types,
//!    bearer tokens, internal query parameters, raw URLs, or server error bodies.
//! 7. **Pure Read-Only Control Surface:** Dispatches only `GET /health`, `GET /model-info`,
//!    `GET /jobs/{handle}`, and `GET /jobs/{handle}/result`. Zero `POST` methods exist in this client.
//! 8. **Safe URL Construction & Exact MIME Essence:** Uses `path_segments_mut` to append fixed
//!    internal routes, preventing origin overwrites. Validates exact MIME type essence.
//! 9. **Pre-Return Binding on Result Downloads:** Result bytes are bounded by client-side
//!    [`ServiceLimits::max_png_bytes`] and verified via [`cleaner_core::cloud_wire::validate_result_bytes`]
//!    before returning.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use cleaner_core::cloud_wire::{
    validate_ascii_id, validate_health_response, validate_job_request_metadata,
    validate_model_info_response, validate_response_binding,
    validate_result_bytes, HealthResponse, JobAcceptedResponse, JobCancelResponse,
    JobRequestMetadata, JobStatusResponse, ModelInfoResponse, ResultMetadata, ServiceLimits,
};
use cleaner_core::engines::render::CloudProvider;
use cleaner_core::cloud_analysis_wire::{AnalysisCapabilities, AnalysisError, AnalysisRequest, AnalysisResult, MAX_RESPONSE_BYTES};
use base64::Engine;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use reqwest::Url;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::inference::config::{
    compute_canonical_endpoint_fingerprint, validate_https_endpoint, validate_profile_id,
};
use crate::inference::secrets::SecretValue;

/// Default connection timeout for cloud inference endpoints (10 seconds).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default total request timeout for control and status endpoints (30 seconds).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default DNS resolution deadline (5 seconds).
pub const DEFAULT_DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum allowable payload size for control JSON responses (1 MiB).
pub const MAX_CONTROL_JSON_BYTES: u64 = 1024 * 1024;

/// Capacity of the global DNS resolver request queue.
const DNS_QUEUE_CAPACITY: usize = 32;

/// Number of concurrent DNS worker threads (>= 4) sharing the bounded queue.
const DNS_WORKER_COUNT: usize = 4;

/// Errors arising from the hardened cloud HTTP transport.
#[derive(Debug, Error, PartialEq)]
pub enum HttpTransportError {
    #[error("invalid target URL configuration")]
    InvalidTarget,

    #[error("invalid profile id")]
    InvalidProfileId,

    #[error("target endpoint validation failed")]
    EndpointValidationFailed,

    #[error("runtime credential missing or invalid for target")]
    InvalidCredential,

    #[error("credential provider does not match target provider")]
    CredentialProviderMismatch,

    #[error("DNS resolver queue busy")]
    DnsResolverBusy,

    #[error("DNS resolution failed for target host")]
    DnsResolutionFailed,

    #[error("DNS resolved to forbidden non-public, reserved, or loopback IP address")]
    NonPublicIpRejected,

    #[error("network connection error")]
    ConnectionError,

    #[error("request timeout exceeded")]
    Timeout,

    #[error("HTTP redirect rejected (redirects are strictly forbidden)")]
    RedirectForbidden,

    #[error("unexpected HTTP status code {status}")]
    UnexpectedStatus { status: u16 },

    #[error("response content-type mismatch")]
    ContentTypeMismatch,

    #[error("response body size exceeded maximum limit")]
    BodySizeLimitExceeded,

    #[error("cloud wire validation failed")]
    WireValidation,


    #[error("JSON deserialization error")]
    JsonDeserialization,

    #[error("provider mismatch in response payload")]
    ProviderMismatch,

    #[error("invalid or malicious job handle identifier")]
    InvalidHandle,
}

fn analysis_rejection_error(status: u16, body: &[u8], expected_digest: &str) -> HttpTransportError {
    let Ok(rejection) = serde_json::from_slice::<AnalysisError>(body) else {
        return HttpTransportError::WireValidation;
    };
    if rejection.validate().is_err()
        || (rejection.request_digest.as_deref() != Some(expected_digest)
            && !(rejection.request_digest.is_none() && matches!(status, 401 | 413 | 503)))
    {
        return HttpTransportError::WireValidation;
    }
    HttpTransportError::UnexpectedStatus { status }
}

/// Validate whether an IP address is a globally reachable public unicast address.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();

    // 0.0.0.0/8: "This network" (RFC 1122)
    if octets[0] == 0 {
        return false;
    }

    // 10.0.0.0/8: Private-Use (RFC 1918)
    if octets[0] == 10 {
        return false;
    }

    // 100.64.0.0/10: Shared Address Space / Carrier-Grade NAT (RFC 6598)
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return false;
    }

    // 127.0.0.0/8: Loopback (RFC 1122)
    if octets[0] == 127 {
        return false;
    }

    // 169.254.0.0/16: Link Local (RFC 3927)
    if octets[0] == 169 && octets[1] == 254 {
        return false;
    }

    // 172.16.0.0/12: Private-Use (RFC 1918)
    if octets[0] == 172 && (octets[1] >= 16 && octets[1] <= 31) {
        return false;
    }

    // 192.0.0.0/24: IETF Protocol Assignments (RFC 6890)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return false;
    }

    // 192.0.2.0/24: TEST-NET-1 (RFC 5737)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return false;
    }

    // 192.88.99.0/24: 6to4 Relay Anycast (RFC 3068 / RFC 7526)
    if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 {
        return false;
    }

    // 192.168.0.0/16: Private-Use (RFC 1918)
    if octets[0] == 192 && octets[1] == 168 {
        return false;
    }

    // 198.18.0.0/15: Network Interconnect Benchmarking (RFC 2544)
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return false;
    }

    // 198.51.100.0/24: TEST-NET-2 (RFC 5737)
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return false;
    }

    // 203.0.113.0/24: TEST-NET-3 (RFC 5737)
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return false;
    }

    // 224.0.0.0/4: Multicast (RFC 5771)
    if octets[0] >= 224 && octets[0] <= 239 {
        return false;
    }

    // 240.0.0.0/4: Reserved for Future Use / Broadcast (RFC 1112)
    if octets[0] >= 240 {
        return false;
    }

    true
}

/// Conservative IPv6 allowlist: permits ONLY global unicast in `2000::/3`, excluding special prefixes.
fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return false; // Documentation prefix, outside 2001::/23.
    }

    // Must be in 2000::/3 (first 3 bits = 001)
    if (segments[0] & 0xe000) != 0x2000 {
        return false;
    }

    // Exclude 2001::/23 (IETF Protocol Assignments, Teredo, Documentation, Benchmarking, ORCHID)
    if segments[0] == 0x2001 && (segments[1] & 0xfe00) == 0 {
        return false;
    }

    // Exclude 2002::/16 (6to4)
    if segments[0] == 0x2002 {
        return false;
    }

    // Exclude 3ffe::/16 and 3fff::/16 (6bone testbed and reserved)
    if segments[0] == 0x3ffe || segments[0] == 0x3fff {
        return false;
    }

    true
}

struct DnsJob {
    host: String,
    port: u16,
    reply: SyncSender<Result<Vec<SocketAddr>, ()>>,
}

static DNS_CHANNEL: OnceLock<SyncSender<DnsJob>> = OnceLock::new();

fn get_dns_sender() -> &'static SyncSender<DnsJob> {
    DNS_CHANNEL.get_or_init(|| {
        let (tx, rx): (SyncSender<DnsJob>, Receiver<DnsJob>) = sync_channel(DNS_QUEUE_CAPACITY);
        let rx_shared = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for i in 0..DNS_WORKER_COUNT {
            let rx_clone = std::sync::Arc::clone(&rx_shared);
            thread::Builder::new()
                .name(format!("cloud-dns-worker-{i}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let rx_lock = match rx_clone.lock() {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                            match rx_lock.recv() {
                                Ok(j) => j,
                                Err(_) => break,
                            }
                        };
                        let addrs = (job.host.as_str(), job.port)
                            .to_socket_addrs()
                            .map(|iter| iter.collect::<Vec<_>>())
                            .map_err(|_| ());
                        let _ = job.reply.send(addrs);
                    }
                })
                .expect("failed to spawn cloud DNS resolver worker");
        }
        tx
    })
}

/// Resolve host addresses with bounded queue dispatch, deadline, and strict public IP validation.
pub fn resolve_and_validate_host(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<Vec<SocketAddr>, HttpTransportError> {
    let sender = get_dns_sender();
    let (reply_tx, reply_rx) = sync_channel(1);

    let job = DnsJob {
        host: host.to_string(),
        port,
        reply: reply_tx,
    };

    // try_send fails immediately if the single worker queue is full
    sender
        .try_send(job)
        .map_err(|_| HttpTransportError::DnsResolverBusy)?;

    let addrs_res = reply_rx
        .recv_timeout(timeout)
        .map_err(|_| HttpTransportError::Timeout)?;

    let addrs = addrs_res.map_err(|_| HttpTransportError::DnsResolutionFailed)?;

    if addrs.is_empty() {
        return Err(HttpTransportError::DnsResolutionFailed);
    }

    for addr in &addrs {
        if !is_public_ip(addr.ip()) {
            return Err(HttpTransportError::NonPublicIpRejected);
        }
    }

    Ok(addrs)
}

/// Distinct vendor credential variants for cloud inference providers.
#[derive(Clone)]
pub enum RuntimeCredential {
    BeamBearer(SecretValue),
    ModalProxy {
        token_id: String,
        token_secret: SecretValue,
    },
}

/// An immutable runtime credential bound strictly to a target provider, profile ID, and origin fingerprint.
#[derive(Clone)]
pub struct BoundRuntimeCredential {
    provider: CloudProvider,
    profile_id: String,
    canonical_origin_fingerprint: String,
    credential: RuntimeCredential,
}

impl BoundRuntimeCredential {
    /// Bind a [`RuntimeCredential`] to a validated [`CloudEndpointTarget`].
    pub fn new(
        target: &CloudEndpointTarget,
        credential: RuntimeCredential,
    ) -> Result<Self, HttpTransportError> {
        match (&credential, target.provider()) {
            (RuntimeCredential::BeamBearer(sec), CloudProvider::Beam) => {
                if sec.is_empty() {
                    return Err(HttpTransportError::InvalidCredential);
                }
            }
            (
                RuntimeCredential::ModalProxy {
                    token_id,
                    token_secret,
                },
                CloudProvider::Modal,
            ) => {
                if token_id.trim().is_empty() || token_secret.is_empty() {
                    return Err(HttpTransportError::InvalidCredential);
                }
            }
            _ => {
                return Err(HttpTransportError::CredentialProviderMismatch);
            }
        }

        Ok(Self {
            provider: target.provider(),
            profile_id: target.profile_id().to_string(),
            canonical_origin_fingerprint: target.canonical_origin_fingerprint().to_string(),
            credential,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixedControlRoute {
    Health,
    ModelInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobSubResource {
    Status,
    Result,
    Cancel,
}

/// An immutable, validated cloud endpoint target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudEndpointTarget {
    provider: CloudProvider,
    profile_id: String,
    base_url: Url,
    canonical_origin: String,
    canonical_origin_fingerprint: String,
    canonical_endpoint_fingerprint: String,
}

impl CloudEndpointTarget {
    /// Construct and validate an immutable cloud endpoint target.
    pub fn new(
        provider: CloudProvider,
        profile_id: &str,
        raw_endpoint_url: &str,
    ) -> Result<Self, HttpTransportError> {
        validate_profile_id(profile_id)
            .map_err(|_| HttpTransportError::InvalidProfileId)?;

        let (canonical_origin, canonical_origin_fingerprint) =
            validate_https_endpoint(raw_endpoint_url)
                .map_err(|_| HttpTransportError::EndpointValidationFailed)?;

        let canonical_endpoint_fingerprint =
            compute_canonical_endpoint_fingerprint(raw_endpoint_url)
                .map_err(|_| HttpTransportError::EndpointValidationFailed)?;

        let mut parsed = Url::parse(raw_endpoint_url)
            .map_err(|_| HttpTransportError::EndpointValidationFailed)?;

        if !parsed.path().ends_with('/') {
            let mut new_path = parsed.path().to_string();
            new_path.push('/');
            parsed.set_path(&new_path);
        }

        Ok(Self {
            provider,
            profile_id: profile_id.to_string(),
            base_url: parsed,
            canonical_origin,
            canonical_origin_fingerprint,
            canonical_endpoint_fingerprint,
        })
    }

    #[cfg(test)]
    /// Internal constructor for unit testing with local mock HTTP servers.
    pub fn new_test_target(
        provider: CloudProvider,
        profile_id: &str,
        raw_endpoint_url: &str,
    ) -> Result<Self, HttpTransportError> {
        let mut parsed = Url::parse(raw_endpoint_url)
            .map_err(|_| HttpTransportError::EndpointValidationFailed)?;

        if !parsed.path().ends_with('/') {
            let mut new_path = parsed.path().to_string();
            new_path.push('/');
            parsed.set_path(&new_path);
        }

        let mock_https = match provider {
            CloudProvider::Beam => "https://api.beam.cloud/mc/v1",
            CloudProvider::Modal => "https://modal.run/mc/v1",
        };
        let (origin, origin_fp) = validate_https_endpoint(mock_https)
            .map_err(|_| HttpTransportError::EndpointValidationFailed)?;
        let endpoint_fp = compute_canonical_endpoint_fingerprint(mock_https)
            .map_err(|_| HttpTransportError::EndpointValidationFailed)?;

        Ok(Self {
            provider,
            profile_id: profile_id.to_string(),
            base_url: parsed,
            canonical_origin: origin,
            canonical_origin_fingerprint: origin_fp,
            canonical_endpoint_fingerprint: endpoint_fp,
        })
    }

    pub fn provider(&self) -> CloudProvider {
        self.provider
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn canonical_origin(&self) -> &str {
        &self.canonical_origin
    }

    pub fn canonical_origin_fingerprint(&self) -> &str {
        &self.canonical_origin_fingerprint
    }

    pub fn canonical_endpoint_fingerprint(&self) -> &str {
        &self.canonical_endpoint_fingerprint
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn build_fixed_route(&self, route: FixedControlRoute) -> Result<Url, HttpTransportError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| HttpTransportError::InvalidTarget)?;
            segments.pop_if_empty();
            match route {
                FixedControlRoute::Health => {
                    segments.push("health");
                }
                FixedControlRoute::ModelInfo => {
                    segments.push("model-info");
                }
            }
        }
        Ok(url)
    }

    fn build_jobs_route(&self) -> Result<Url, HttpTransportError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| HttpTransportError::InvalidTarget)?;
            segments.pop_if_empty();
            segments.push("jobs");
        }
        Ok(url)
    }

    fn build_job_route(
        &self,
        handle: &str,
        sub: JobSubResource,
    ) -> Result<Url, HttpTransportError> {
        validate_ascii_id(handle, "handle", 1, 128)
            .map_err(|_| HttpTransportError::WireValidation)?;

        if handle == "."
            || handle == ".."
            || handle.contains('/')
            || handle.contains('\\')
            || handle.contains('%')
            || handle.contains('?')
            || handle.contains('#')
            || handle.contains(':')
        {
            return Err(HttpTransportError::InvalidHandle);
        }

        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| HttpTransportError::InvalidTarget)?;
            segments.pop_if_empty();
            segments.push("jobs");
            segments.push(handle);
            match sub {
                JobSubResource::Status => {}
                JobSubResource::Result => {
                    segments.push("result");
                }
                JobSubResource::Cancel => {
                    segments.push("cancel");
                }
            }
        }
        Ok(url)
    }
}

fn build_multipart_body(
    boundary: &str,
    metadata_json: &[u8],
    image_png: &[u8],
    hint_png: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(metadata_json.len() + image_png.len() + hint_png.len() + 512);

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"metadata\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(metadata_json);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"image\"; filename=\"image.png\"\r\n");
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(image_png);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"hint\"; filename=\"hint.png\"\r\n");
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(hint_png);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// Hardened HTTP transport client for cloud inference read-only control and execution endpoints.
#[derive(Clone)]
pub struct CloudHttpClient {
    target: CloudEndpointTarget,
    client: Client,
    credential: BoundRuntimeCredential,
}

impl CloudHttpClient {
    fn analysis_url(&self, route: &str) -> Url {
        let mut url = self.target.base_url.clone();
        url.set_path(&format!("/mc/analysis/v1/{route}"));
        url
    }

    pub fn get_analysis_capabilities(&self) -> Result<AnalysisCapabilities, HttpTransportError> {
        let capabilities: AnalysisCapabilities = self.send_get_json(
            self.analysis_url("capabilities"), MAX_CONTROL_JSON_BYTES,
        )?;
        capabilities.validate().map_err(|_| HttpTransportError::WireValidation)?;
        Ok(capabilities)
    }

    pub fn submit_analysis_tile(
        &self,
        request: &AnalysisRequest,
        tile_png: &[u8],
    ) -> Result<AnalysisResult, HttpTransportError> {
        request.validate(tile_png).map_err(|_| HttpTransportError::WireValidation)?;
        let envelope = serde_json::json!({
            "metadata": request,
            "tile_png_b64": base64::engine::general_purpose::STANDARD.encode(tile_png),
        });
        let body = serde_json::to_vec(&envelope).map_err(|_| HttpTransportError::WireValidation)?;
        if body.len() > 8_000_000 { return Err(HttpTransportError::BodySizeLimitExceeded); }
        let req = self.client.post(self.analysis_url("analyze"))
            // A scale-to-zero GPU may need a cold start before its first ONNX
            // tile. Keep the regular control/job calls at their 30 s bound.
            .timeout(Duration::from_secs(450))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(body);
        let response = self.attach_auth(req)?.send().map_err(|_| HttpTransportError::ConnectionError)?;
        let status = response.status();
        if status.is_redirection() || response.headers().contains_key(LOCATION) {
            return Err(HttpTransportError::RedirectForbidden);
        }
        Self::verify_content_type(&response, "application/json")?;
        let bytes = Self::read_bounded_body(response, MAX_RESPONSE_BYTES as u64)?;
        if !status.is_success() {
            return Err(analysis_rejection_error(status.as_u16(), &bytes, &request.request_digest));
        }
        let result: AnalysisResult = serde_json::from_slice(&bytes)
            .map_err(|_| HttpTransportError::JsonDeserialization)?;
        result.validate(request).map_err(|_| HttpTransportError::WireValidation)?;
        Ok(result)
    }

    /// Create a new hardened cloud HTTP client with a bound runtime credential.
    pub fn new(
        target: CloudEndpointTarget,
        credential: BoundRuntimeCredential,
    ) -> Result<Self, HttpTransportError> {
        if credential.provider != target.provider
            || credential.profile_id != target.profile_id
            || credential.canonical_origin_fingerprint != target.canonical_origin_fingerprint
        {
            return Err(HttpTransportError::InvalidCredential);
        }

        let host = target
            .base_url
            .host_str()
            .ok_or(HttpTransportError::InvalidTarget)?;
        let port = target
            .base_url
            .port_or_known_default()
            .unwrap_or(443);

        let pinned_addrs = resolve_and_validate_host(host, port, DEFAULT_DNS_TIMEOUT)?;

        let mut builder = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .https_only(true);

        builder = builder.resolve_to_addrs(host, &pinned_addrs);

        let client = builder
            .build()
            .map_err(|_| HttpTransportError::ConnectionError)?;

        Ok(Self {
            target,
            client,
            credential,
        })
    }

    #[cfg(test)]
    /// Create test client with injected reqwest client (for mock loopback testing).
    pub fn new_test_client(
        target: CloudEndpointTarget,
        credential: BoundRuntimeCredential,
        client: Client,
    ) -> Self {
        Self {
            target,
            client,
            credential,
        }
    }

    pub fn target(&self) -> &CloudEndpointTarget {
        &self.target
    }

    fn read_bounded_body(
        mut response: Response,
        max_bytes: u64,
    ) -> Result<Vec<u8>, HttpTransportError> {
        let mut reader = (&mut response).take(max_bytes.saturating_add(1));
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .map_err(|_| HttpTransportError::ConnectionError)?;

        if buf.len() as u64 > max_bytes {
            return Err(HttpTransportError::BodySizeLimitExceeded);
        }

        Ok(buf)
    }

    fn verify_content_type(
        response: &Response,
        expected_essence: &str,
    ) -> Result<(), HttpTransportError> {
        let raw_ct = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let essence = raw_ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        if essence != expected_essence {
            return Err(HttpTransportError::ContentTypeMismatch);
        }

        Ok(())
    }

    fn attach_auth(&self, mut builder: RequestBuilder) -> Result<RequestBuilder, HttpTransportError> {
        match &self.credential.credential {
            RuntimeCredential::BeamBearer(secret) => {
                let token_str = secret
                    .expose_str()
                    .map_err(|_| HttpTransportError::InvalidCredential)?;
                let mut val = HeaderValue::from_str(&format!("Bearer {token_str}"))
                    .map_err(|_| HttpTransportError::InvalidCredential)?;
                val.set_sensitive(true);
                builder = builder.header(AUTHORIZATION, val);
            }
            RuntimeCredential::ModalProxy {
                token_id,
                token_secret,
            } => {
                let secret_str = token_secret
                    .expose_str()
                    .map_err(|_| HttpTransportError::InvalidCredential)?;
                let mut key_val = HeaderValue::from_str(token_id)
                    .map_err(|_| HttpTransportError::InvalidCredential)?;
                key_val.set_sensitive(true);
                let mut secret_val = HeaderValue::from_str(secret_str)
                    .map_err(|_| HttpTransportError::InvalidCredential)?;
                secret_val.set_sensitive(true);

                builder = builder.header("Modal-Key", key_val);
                builder = builder.header("Modal-Secret", secret_val);
            }
        }
        Ok(builder)
    }

    fn send_get_json<T: DeserializeOwned>(
        &self,
        url: Url,
        max_bytes: u64,
    ) -> Result<T, HttpTransportError> {
        let req = self.client.get(url).header(ACCEPT, "application/json");
        let prepared = self.attach_auth(req)?;

        let resp = prepared
            .send()
            .map_err(|_| HttpTransportError::ConnectionError)?;

        let status = resp.status();
        if status.is_redirection() || resp.headers().contains_key(LOCATION) {
            return Err(HttpTransportError::RedirectForbidden);
        }

        if !status.is_success() {
            return Err(HttpTransportError::UnexpectedStatus {
                status: status.as_u16(),
            });
        }

        Self::verify_content_type(&resp, "application/json")?;
        let body_bytes = Self::read_bounded_body(resp, max_bytes)?;

        serde_json::from_slice::<T>(&body_bytes)
            .map_err(|_| HttpTransportError::JsonDeserialization)
    }

    /// Authenticated health check (`GET /mc/v1/health`).
    pub fn get_health(&self) -> Result<HealthResponse, HttpTransportError> {
        let url = self.target.build_fixed_route(FixedControlRoute::Health)?;
        let resp: HealthResponse = self.send_get_json(url, MAX_CONTROL_JSON_BYTES)?;

        validate_health_response(&resp).map_err(|_| HttpTransportError::WireValidation)?;

        if resp.provider != self.target.provider {
            return Err(HttpTransportError::ProviderMismatch);
        }

        Ok(resp)
    }

    /// Authenticated model-info runtime capability check (`GET /mc/v1/model-info`).
    pub fn get_model_info(&self) -> Result<ModelInfoResponse, HttpTransportError> {
        let url = self.target.build_fixed_route(FixedControlRoute::ModelInfo)?;
        let resp: ModelInfoResponse = self.send_get_json(url, MAX_CONTROL_JSON_BYTES)?;

        validate_model_info_response(&resp).map_err(|_| HttpTransportError::WireValidation)?;

        if resp.provider != self.target.provider {
            return Err(HttpTransportError::ProviderMismatch);
        }

        Ok(resp)
    }

    /// Authenticated job status check bound to expected handle and request metadata (`GET /mc/v1/jobs/{handle}`).
    pub fn get_job_status(
        &self,
        handle: &str,
        request_meta: &JobRequestMetadata,
    ) -> Result<JobStatusResponse, HttpTransportError> {
        validate_job_request_metadata(request_meta).map_err(|_| HttpTransportError::WireValidation)?;

        let url = self.target.build_job_route(handle, JobSubResource::Status)?;
        let resp: JobStatusResponse = self.send_get_json(url, MAX_CONTROL_JSON_BYTES)?;

        validate_response_binding(request_meta, &resp, Some(handle))
            .map_err(|_| HttpTransportError::WireValidation)?;

        Ok(resp)
    }

    /// Fetch raw result PNG bytes from protected result endpoint (`GET /mc/v1/jobs/{handle}/result`).
    ///
    /// Download bytes are capped by client [`ServiceLimits::max_png_bytes`] and verified
    /// against [`validate_result_bytes`] before returning.
    pub fn fetch_result_bytes(
        &self,
        handle: &str,
        result_meta: &ResultMetadata,
        request_meta: &JobRequestMetadata,
        limits: &ServiceLimits,
    ) -> Result<Vec<u8>, HttpTransportError> {
        let url = self.target.build_job_route(handle, JobSubResource::Result)?;
        let req = self.client.get(url).header(ACCEPT, "image/png");
        let prepared = self.attach_auth(req)?;

        let resp = prepared
            .send()
            .map_err(|_| HttpTransportError::ConnectionError)?;

        let status = resp.status();
        if status.is_redirection() || resp.headers().contains_key(LOCATION) {
            return Err(HttpTransportError::RedirectForbidden);
        }

        if !status.is_success() {
            return Err(HttpTransportError::UnexpectedStatus {
                status: status.as_u16(),
            });
        }

        Self::verify_content_type(&resp, "image/png")?;
        let bytes = Self::read_bounded_body(resp, limits.max_png_bytes.min(cleaner_core::cloud_decode::MAX_RESULT_ENCODED_BYTES))?;

        validate_result_bytes(&bytes, result_meta, request_meta, limits, handle)
            .map_err(|_| HttpTransportError::WireValidation)?;

        Ok(bytes)
    }

    /// Submit a crop rendering job (`POST /mc/v1/jobs`).
    pub fn submit_job(
        &self,
        request_meta: &JobRequestMetadata,
        image_bytes: &[u8],
        hint_bytes: &[u8],
        limits: &ServiceLimits,
    ) -> Result<JobAcceptedResponse, HttpTransportError> {
        cleaner_core::cloud_wire::validate_crop_payload(
            request_meta,
            image_bytes,
            hint_bytes,
            limits,
        )
        .map_err(|_| HttpTransportError::WireValidation)?;

        let url = self.target.build_jobs_route()?;
        let meta_json = serde_json::to_vec(request_meta)
            .map_err(|_| HttpTransportError::WireValidation)?;

        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let boundary = format!("------------------------manga_cleaner_{}_{}", nanos, count);
        let body = build_multipart_body(&boundary, &meta_json, image_bytes, hint_bytes);

        let ct = format!("multipart/form-data; boundary={boundary}");
        let req = self
            .client
            .post(url)
            .header(CONTENT_TYPE, ct)
            .header(ACCEPT, "application/json")
            .body(body);

        let prepared = self.attach_auth(req)?;
        let resp = prepared
            .send()
            .map_err(|_| HttpTransportError::ConnectionError)?;

        let status = resp.status();
        if status.is_redirection() || resp.headers().contains_key(LOCATION) {
            return Err(HttpTransportError::RedirectForbidden);
        }

        if status == reqwest::StatusCode::ACCEPTED {
            Self::verify_content_type(&resp, "application/json")?;
            let body_bytes = Self::read_bounded_body(resp, MAX_CONTROL_JSON_BYTES)?;
            let accepted: JobAcceptedResponse = serde_json::from_slice(&body_bytes)
                .map_err(|_| HttpTransportError::JsonDeserialization)?;
            validate_response_binding(request_meta, &accepted, None)
                .map_err(|_| HttpTransportError::WireValidation)?;
            return Ok(accepted);
        }

        // Non-202 status: check for structured pre-enqueue rejection
        if resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.contains("application/json"))
            .unwrap_or(false)
        {
            if let Ok(body_bytes) = Self::read_bounded_body(resp, MAX_CONTROL_JSON_BYTES) {
                if let Ok(rejection) = serde_json::from_slice::<
                    cleaner_core::cloud_wire::PreEnqueueRejectionResponse,
                >(&body_bytes) {
                    let _ = cleaner_core::cloud_wire::validate_pre_enqueue_rejection(&rejection);
                }
            }
        }

        Err(HttpTransportError::UnexpectedStatus {
            status: status.as_u16(),
        })
    }

    /// Request job cancellation (`POST /mc/v1/jobs/{handle}/cancel`).
    pub fn cancel_job(
        &self,
        handle: &str,
        request_meta: &JobRequestMetadata,
    ) -> Result<JobCancelResponse, HttpTransportError> {
        let url = self.target.build_job_route(handle, JobSubResource::Cancel)?;
        let req = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body("{}");

        let prepared = self.attach_auth(req)?;
        let resp = prepared
            .send()
            .map_err(|_| HttpTransportError::ConnectionError)?;

        let status = resp.status();
        if status.is_redirection() || resp.headers().contains_key(LOCATION) {
            return Err(HttpTransportError::RedirectForbidden);
        }

        if !status.is_success() {
            return Err(HttpTransportError::UnexpectedStatus {
                status: status.as_u16(),
            });
        }

        Self::verify_content_type(&resp, "application/json")?;
        let body_bytes = Self::read_bounded_body(resp, MAX_CONTROL_JSON_BYTES)?;
        let cancel_resp: JobCancelResponse = serde_json::from_slice(&body_bytes)
            .map_err(|_| HttpTransportError::JsonDeserialization)?;

        cleaner_core::cloud_wire::validate_cancel_response(request_meta, &cancel_resp, handle)
            .map_err(|_| HttpTransportError::WireValidation)?;

        Ok(cancel_resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn analysis_unauthorized_rejection_preserves_status() {
        let body = include_bytes!("../../../deploy/cloud/fixtures/analysis_v1/unauthorized_error.json");
        assert_eq!(
            analysis_rejection_error(401, body, &"a".repeat(64)),
            HttpTransportError::UnexpectedStatus { status: 401 },
        );
        assert_eq!(
            analysis_rejection_error(400, body, &"a".repeat(64)),
            HttpTransportError::WireValidation,
        );
        let unavailable = include_bytes!("../../../deploy/cloud/fixtures/analysis_v1/capability_unavailable_error.json");
        assert_eq!(
            analysis_rejection_error(503, unavailable, &"a".repeat(64)),
            HttpTransportError::UnexpectedStatus { status: 503 },
        );
        let failed = include_bytes!("../../../deploy/cloud/fixtures/analysis_v1/inference_failed_error.json");
        let digest = "1dffe149aed278e23eff1a782d51f896a3a1a658e78732f6173a1eb305e74255";
        assert_eq!(
            analysis_rejection_error(500, failed, digest),
            HttpTransportError::UnexpectedStatus { status: 500 },
        );
        assert_eq!(
            analysis_rejection_error(500, failed, &"a".repeat(64)),
            HttpTransportError::WireValidation,
        );
    }
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn test_public_ip_classifier_ipv4() {
        // Forbidden: Loopback
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(127, 255, 255, 255))));

        // Forbidden: Private ranges (RFC 1918)
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 254))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));

        // Forbidden: Link local / Cloud metadata
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));

        // Forbidden: CGNAT (100.64.0.0/10)
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));

        // Forbidden: Documentation & Benchmarking
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));

        // Forbidden: Multicast & Future Reserved
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255))));

        // Allowed: Public IPs
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn test_public_ip_classifier_ipv6_conservative() {
        // Forbidden: Outside 2000::/3
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(!is_public_ip(IpAddr::V6("fe80::1".parse().unwrap())));
        assert!(!is_public_ip(IpAddr::V6("fec0::1".parse().unwrap())));
        assert!(!is_public_ip(IpAddr::V6("fc00::1".parse().unwrap())));
        assert!(!is_public_ip(IpAddr::V6("ff02::1".parse().unwrap())));
        assert!(!is_public_ip(IpAddr::V6("100::1".parse().unwrap())));

        // Forbidden: Special ranges within 2000::/3
        assert!(!is_public_ip(IpAddr::V6("2001:db8::1".parse().unwrap()))); // Documentation
        assert!(!is_public_ip(IpAddr::V6("2001:2::1".parse().unwrap()))); // Benchmarking
        assert!(!is_public_ip(IpAddr::V6("2001::1".parse().unwrap()))); // Teredo
        assert!(!is_public_ip(IpAddr::V6("2002:c0a8:101::1".parse().unwrap()))); // 6to4
        assert!(!is_public_ip(IpAddr::V6("3ffe::1".parse().unwrap()))); // 6bone

        // Allowed: Global 2000::/3
        assert!(is_public_ip(IpAddr::V6("2606:4700:4700::1111".parse().unwrap())));
        assert!(is_public_ip(IpAddr::V6("2a00:1450:4009:81f::200e".parse().unwrap())));
    }

    #[test]
    fn test_cloud_endpoint_target_fixed_routes_and_handle_rejection() {
        let target = CloudEndpointTarget::new(
            CloudProvider::Modal,
            "modal-prod",
            "https://modal-cleaner.run.modal.com/mc/v1",
        )
        .expect("valid target");

        assert_eq!(
            target.build_fixed_route(FixedControlRoute::Health).unwrap().as_str(),
            "https://modal-cleaner.run.modal.com/mc/v1/health"
        );
        assert_eq!(
            target.build_fixed_route(FixedControlRoute::ModelInfo).unwrap().as_str(),
            "https://modal-cleaner.run.modal.com/mc/v1/model-info"
        );
        assert_eq!(
            target.build_job_route("handle-123", JobSubResource::Status).unwrap().as_str(),
            "https://modal-cleaner.run.modal.com/mc/v1/jobs/handle-123"
        );
        assert_eq!(
            target.build_job_route("handle-123", JobSubResource::Result).unwrap().as_str(),
            "https://modal-cleaner.run.modal.com/mc/v1/jobs/handle-123/result"
        );

        // Dotsegments and URL injection rejected
        assert_eq!(
            target.build_job_route("..", JobSubResource::Status),
            Err(HttpTransportError::InvalidHandle)
        );
        assert_eq!(
            target.build_job_route(".", JobSubResource::Status),
            Err(HttpTransportError::InvalidHandle)
        );
        assert_eq!(
            target.build_job_route("handle/with/slash", JobSubResource::Status),
            Err(HttpTransportError::InvalidHandle)
        );
        assert_eq!(
            target.build_job_route("handle%20space", JobSubResource::Status),
            Err(HttpTransportError::InvalidHandle)
        );
    }

    #[test]
    fn test_credential_provider_binding() {
        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-1",
            "https://localhost.test/mc/v1",
        )
        .unwrap();

        // Mismatched provider variant
        let beam_cred = RuntimeCredential::BeamBearer(SecretValue::new("beam-token"));
        let err = BoundRuntimeCredential::new(&target, beam_cred).err().unwrap();
        assert_eq!(err, HttpTransportError::CredentialProviderMismatch);

        // Valid ModalProxy variant
        let modal_cred = RuntimeCredential::ModalProxy {
            token_id: "tid-123".to_string(),
            token_secret: SecretValue::new("tsec-456"),
        };
        let bound = BoundRuntimeCredential::new(&target, modal_cred).unwrap();
        assert_eq!(bound.provider, CloudProvider::Modal);
    }

    #[test]
    fn test_loopback_mock_redirect_forbidden() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let response = "HTTP/1.1 302 Found\r\nLocation: https://evil.com/leak\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Beam,
            "beam-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::BeamBearer(SecretValue::new("token")),
        )
        .unwrap();

        let client = CloudHttpClient::new_test_client(target, cred, Client::builder().redirect(Policy::none())
            .retry(reqwest::retry::never()).build().unwrap());
        let err = client.get_health().unwrap_err();
        assert_eq!(err, HttpTransportError::RedirectForbidden);
    }

    #[test]
    fn test_loopback_mock_chunked_over_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request before closing the response socket: unread request
                // bytes can cause a TCP reset and hide the intended body-limit error.
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 8192);
                }
                let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = stream.write_all(header.as_bytes());
                // Emit huge chunk > 1MB
                let chunk_size = MAX_CONTROL_JSON_BYTES + 100;
                let chunk_header = format!("{:x}\r\n", chunk_size);
                let _ = stream.write_all(chunk_header.as_bytes());
                let payload = vec![b'a'; chunk_size as usize];
                let _ = stream.write_all(&payload);
                let _ = stream.write_all(b"\r\n0\r\n\r\n");
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Beam,
            "beam-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::BeamBearer(SecretValue::new("token")),
        )
        .unwrap();

        let client = CloudHttpClient::new_test_client(target, cred, Client::new());
        let err = client.get_health().unwrap_err();
        assert_eq!(err, HttpTransportError::BodySizeLimitExceeded);
    }

    #[test]
    fn test_loopback_mock_mime_essence_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Return invalid MIME extension like application/json-evil or text/html
                let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json-evil; charset=utf-8\r\nContent-Length: 2\r\n\r\n{}";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Beam,
            "beam-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::BeamBearer(SecretValue::new("token")),
        )
        .unwrap();

        let client = CloudHttpClient::new_test_client(target, cred, Client::new());
        let err = client.get_health().unwrap_err();
        assert_eq!(err, HttpTransportError::ContentTypeMismatch);
    }

    #[test]
    fn test_loopback_mock_auth_headers_and_status_binding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let req_str = String::from_utf8_lossy(&buf);

                // Verify Modal-Key and Modal-Secret headers arrived
                assert!(req_str.contains("modal-key: test-modal-key"));
                assert!(req_str.contains("modal-secret: test-modal-sec"));

                // Return status response with wrong job_id
                let json = r#"{
                    "handle": "h-123",
                    "job_id": "wrong-job-id",
                    "attempt_id": "att-1",
                    "request_digest": "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0",
                    "recipe_id": "test-sdnq-v1",
                    "preprocessing_version": "1.0.0",
                    "model_id": "test-flux-schnell",
                    "model_revision": "0123456789abcdef0123456789abcdef01234567",
                    "native_mask_conditioning": false,
                    "status": "running"
                }"#;
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::ModalProxy {
                token_id: "test-modal-key".to_string(),
                token_secret: SecretValue::new("test-modal-sec"),
            },
        )
        .unwrap();

        let req_meta = JobRequestMetadata {
            protocol_version: "1.0.0".to_string(),
            job_id: "expected-job-id".to_string(),
            attempt_id: "att-1".to_string(),
            recipe: cleaner_core::cloud_wire::WireRenderRecipe {
                recipe_id: "test-sdnq-v1".to_string(),
                preprocessing_version: "1.0.0".to_string(),
                model_id: "test-flux-schnell".to_string(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
                native_mask_conditioning: false,
            },
            width: 16,
            height: 16,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "2e0a9a2d5f28563463f634d5b81ec2ef90d91afe472a0c6e2c6ef4082f93ec5f".to_string(),
            hint_sha256: "3552bb8bf493ba63896f7abfb3ee3d899bf7312cd0b3d87691a639344b051238".to_string(),
            request_digest: "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0".to_string(),
        };

        let client = CloudHttpClient::new_test_client(target, cred, Client::new());
        let err = client.get_job_status("h-123", &req_meta).unwrap_err();
        assert!(matches!(err, HttpTransportError::WireValidation));
    }

    #[test]
    fn test_loopback_mock_submit_job_accepted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);

                let req_str = String::from_utf8_lossy(&buf);
                let digest = if let Some(idx) = req_str.find("\"request_digest\":\"") {
                    let rest = &req_str[idx + 18..];
                    if let Some(end) = rest.find('"') {
                        &rest[..end]
                    } else {
                        "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0"
                    }
                } else {
                    "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0"
                };

                let json = format!(r#"{{
                    "handle": "h-test-456",
                    "status": "pending",
                    "job_id": "job-sub-1",
                    "attempt_id": "att-sub-1",
                    "request_digest": "{digest}",
                    "recipe_id": "test-sdnq-v1",
                    "preprocessing_version": "1.0.0",
                    "model_id": "test-flux-schnell",
                    "model_revision": "0123456789abcdef0123456789abcdef01234567",
                    "native_mask_conditioning": false
                }}"#);
                let response = format!("HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Beam,
            "beam-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::BeamBearer(SecretValue::new("test-token")),
        )
        .unwrap();

        let req_meta = JobRequestMetadata {
            protocol_version: "1.0.0".to_string(),
            job_id: "job-sub-1".to_string(),
            attempt_id: "att-sub-1".to_string(),
            recipe: cleaner_core::cloud_wire::WireRenderRecipe {
                recipe_id: "test-sdnq-v1".to_string(),
                preprocessing_version: "1.0.0".to_string(),
                model_id: "test-flux-schnell".to_string(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
                native_mask_conditioning: false,
            },
            width: 16,
            height: 16,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "2e0a9a2d5f28563463f634d5b81ec2ef90d91afe472a0c6e2c6ef4082f93ec5f".to_string(),
            hint_sha256: "3552bb8bf493ba63896f7abfb3ee3d899bf7312cd0b3d87691a639344b051238".to_string(),
            request_digest: "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0".to_string(),
        };

        let limits = cleaner_core::cloud_wire::provisional_fixture_limits();
        let client = CloudHttpClient::new_test_client(target, cred, Client::new());

        // Dummy PNGs matching hashes:
        // Notice: to pass validate_crop_payload, the PNG headers and digests must match!
        let raster_rgb = cleaner_core::image::Raster {
            width: 16,
            height: 16,
            mode: cleaner_core::image::ColorMode::Rgb,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![128; 16 * 16 * 3],
        };
        let img_bytes = cleaner_core::image::encode(&raster_rgb, cleaner_core::image::Format::Png).unwrap();
        let raster_gray = cleaner_core::image::Raster {
            width: 16,
            height: 16,
            mode: cleaner_core::image::ColorMode::Gray,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![255; 16 * 16],
        };
        let hint_bytes = cleaner_core::image::encode(&raster_gray, cleaner_core::image::Format::Png).unwrap();

        let mut req_meta_valid = req_meta.clone();
        req_meta_valid.image_sha256 = cleaner_core::ingest::sha256_hex(&img_bytes);
        req_meta_valid.hint_sha256 = cleaner_core::ingest::sha256_hex(&hint_bytes);
        req_meta_valid.request_digest = cleaner_core::cloud_wire::compute_request_digest(&req_meta_valid);

        let accepted = client.submit_job(&req_meta_valid, &img_bytes, &hint_bytes, &limits).unwrap();
        assert_eq!(accepted.handle, "h-test-456");
        assert_eq!(accepted.status, cleaner_core::cloud_wire::JobExecutionStatus::Pending);
    }

    #[test]
    fn test_loopback_mock_cancel_job() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);

                let json = r#"{
                    "handle": "h-cancel-1",
                    "job_id": "job-can-1",
                    "attempt_id": "att-can-1",
                    "request_digest": "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0",
                    "status": "cancel_requested",
                    "acknowledged": true
                }"#;
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let target = CloudEndpointTarget::new_test_target(
            CloudProvider::Beam,
            "beam-1",
            &format!("http://127.0.0.1:{port}/mc/v1"),
        )
        .unwrap();

        let cred = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::BeamBearer(SecretValue::new("test-token")),
        )
        .unwrap();

        let req_meta = JobRequestMetadata {
            protocol_version: "1.0.0".to_string(),
            job_id: "job-can-1".to_string(),
            attempt_id: "att-can-1".to_string(),
            recipe: cleaner_core::cloud_wire::WireRenderRecipe {
                recipe_id: "test-sdnq-v1".to_string(),
                preprocessing_version: "1.0.0".to_string(),
                model_id: "test-flux-schnell".to_string(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
                native_mask_conditioning: false,
            },
            width: 16,
            height: 16,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "2e0a9a2d5f28563463f634d5b81ec2ef90d91afe472a0c6e2c6ef4082f93ec5f".to_string(),
            hint_sha256: "3552bb8bf493ba63896f7abfb3ee3d899bf7312cd0b3d87691a639344b051238".to_string(),
            request_digest: "42174589391d9064c743c4450b520f403a3b91171382c4eadc9934285f21efc0".to_string(),
        };

        let client = CloudHttpClient::new_test_client(target, cred, Client::new());
        let cancel_resp = client.cancel_job("h-cancel-1", &req_meta).unwrap();
        assert_eq!(cancel_resp.handle, "h-cancel-1");
        assert_eq!(cancel_resp.status, "cancel_requested");
        assert!(cancel_resp.acknowledged);
    }

    #[test]
    fn test_dns_worker_pool_concurrent_resolutions_and_ssrf_rejection() {
        // Loopback / non-public rejected fail-closed
        let err = resolve_and_validate_host("127.0.0.1", 443, Duration::from_secs(2)).unwrap_err();
        assert_eq!(err, HttpTransportError::NonPublicIpRejected);

        // Multiple concurrent resolutions across workers
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(thread::spawn(|| {
                // Loopback IP resolution fails closed consistently
                let res = resolve_and_validate_host("127.0.0.1", 443, Duration::from_secs(2));
                assert_eq!(res, Err(HttpTransportError::NonPublicIpRejected));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    /// Manual live transport probe. The caller supplies secrets only in the
    /// child process environment; this test prints no credential material.
    #[test]
    #[ignore = "requires MC_LIVE_MODAL_ENDPOINT, MC_LIVE_MODAL_TOKEN_ID and MC_LIVE_MODAL_TOKEN_SECRET"]
    fn live_modal_native_control_and_analysis_capabilities() {
        let endpoint = std::env::var("MC_LIVE_MODAL_ENDPOINT").expect("live endpoint");
        let token_id = std::env::var("MC_LIVE_MODAL_TOKEN_ID").expect("live token id");
        let token_secret = std::env::var("MC_LIVE_MODAL_TOKEN_SECRET").expect("live token secret");
        let target = CloudEndpointTarget::new(CloudProvider::Modal, "mc-live-probe", &endpoint)
            .expect("valid live Modal endpoint");
        let credential = BoundRuntimeCredential::new(
            &target,
            RuntimeCredential::ModalProxy {
                token_id,
                token_secret: SecretValue::new(&token_secret),
            },
        ).expect("bound Modal credential");
        let client = CloudHttpClient::new(target, credential).expect("native transport");
        let health = client.get_health().expect("authenticated health");
        assert_eq!(health.status, "ok");
        let model = client.get_model_info().expect("pinned model info");
        assert!(model.model_id.contains("FLUX.2-klein"));
        let analysis = client.get_analysis_capabilities().expect("analysis capabilities");
        assert!(analysis.capabilities.iter().any(|item| item.capability == "text_mask_sam_ts@1"));
        assert!(analysis.capabilities.iter().any(|item| item.capability == "text_regions_rt@1"));
        let rt = analysis.capabilities.iter().find(|item| item.capability == "text_regions_rt@1")
            .expect("RT capability");
        let tile_png = include_bytes!("../../../deploy/cloud/fixtures/tiny_image.png");
        let mut request = AnalysisRequest {
            protocol_version: cleaner_core::cloud_analysis_wire::VERSION.into(),
            capability: rt.capability.clone(),
            graph_sha256s: rt.graph_sha256s.clone(),
            model_revision: rt.model_revision.clone(),
            tile_id: "native-rt-probe".into(),
            tile_rect: cleaner_core::cloud_analysis_wire::TileRect {
                x: 0, y: 0, width: 16, height: 16,
            },
            tile_png_sha256: format!("{:x}", sha2::Sha256::digest(tile_png)),
            source_page_sha256: format!("{:x}", sha2::Sha256::digest(tile_png)),
            request_digest: String::new(),
        };
        request.request_digest = request.digest().expect("request digest");
        let result = client.submit_analysis_tile(&request, tile_png).expect("native RT tile");
        assert_eq!(result.capability, rt.capability);
        println!("native Modal control passed: {} and {} analysis capabilities",
            model.model_id, analysis.capabilities.len());
    }
}
