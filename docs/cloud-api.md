# Manga Cleaner Cloud Wire API Specification (`/mc/v1`)

**Protocol Version:** `1.0.0` (offline contract)
**Status:** P0 architecture notes, superseded for exact schemas by [cloud-contract.md](cloud-contract.md). No deployed gateway or paid execution is available.

---

## 1. Core Principles & Boundary Invariants

1. **Crop-Only Transmission & Zero Coordinate Leakage:** Only cropped raster bounds grown by context padding are transmitted. Never send full pages, source/canvas coordinates (`x`, `y`, `on_page`, `strip_rect`), project archives, or local filesystem paths.
2. **Provider-Agnostic Gateway:** Future Modal and Beam wrappers must expose this uniform `/mc/v1` REST interface. Provider-native container lifecycle and queuing details are encapsulated behind the gateway.
3. **No Blind Deduplication Guarantee:** The client-computed `request_digest` provides content and metadata integrity only. The server does not maintain an idempotency index or offer a digest-lookup endpoint.
4. **Host-Side Composition & Bounded Validation:** The cloud runner executes the diffusion recipe and returns an RGB patch. Encoded byte limits and decoded raster bounds are enforced before allocation/decode, followed by exact geometry validation. Context alpha blending, edge replication, and tone matching remain strictly local host-side operations in Rust.
5. **Diffusion Recipe Realities:** Reference implementation is the FLUX SDNQ recipe from `sidecar/manga_cleaner_sidecar/backend/sdnq.py` (`WORK_LONG_SIDE = 768`, `LATENT_STRIDE = 16`). Current diffusion pipelines lack native masked diffusion; grayscale hints are advisory and ignored by the network. Fixed prompt and hyperparameter allowlists prevent arbitrary prompt/code execution.

---

## 2. Canonical Request Digest & Identifiers

The P0 digest proposal is superseded by the ordered JSON array in
[cloud-contract.md](cloud-contract.md). The current protocol is `1.0.0`;
`cloud_wire.rs`, `contract.py`, and shared fixtures define its exact bytes.
Do not implement the former JCS formula or delimiter concatenation.

The digest binds content and recipe metadata. It is neither authorization nor
an idempotency guarantee. Every dispatch still needs a backend grant, and an
ambiguous submission must never be retried automatically.

---

## 3. Wire Endpoints (All Authenticated)

Every request requires authenticated transport headers adapted by the Rust client to the target provider (e.g. Modal proxy token ID + secret via proxy headers, or Beam runtime authentication tokens). Common unverified bearer claims are prohibited.

### `GET /mc/v1/health`
Proves control-plane reachability and gateway authentication only. Does NOT verify GPU allocation, model residency, or container warmup. Returns gateway status and provider identifier.

### `GET /mc/v1/model-info`
Returns authenticated runtime metadata: supported protocol version, pinned symbolic model identifier `<pinned-model-id>`, pinned revision `<pinned-model-revision>`, pinned recipe `<pinned-recipe-id>`, and provisional service limits.
- **Provisional Limits (Unmeasured):**
  - Max dimensions: 2048×2048 px; Max megapixels: 4.19 MP.
  - Fixture max PNG: 16 MiB; fixture max multipart: 32 MiB; Default worker deadline: 120s.
  *(These schema-only draft limits are provisional and unmeasured; they must NOT enable cloud execution until measured in P1/P5 benchmarking. Shared offline wire fixtures exist; production limits remain unmeasured).*

### `POST /mc/v1/jobs`
Accepts `multipart/form-data` containing:
- `metadata` (canonical JSON): includes `protocol_version`, `job_id`, `attempt_id`, `request_digest`, crop dimensions, and allowlisted recipe settings.
- `image` (PNG, RGB8, exact snapped dimensions).
- `hint` (PNG, Gray8, exact snapped dimensions).
- **Response `202 Accepted`:** Returns `{ "handle": "<opaque-handle>", "status": "pending", "request_digest": "..." }`.

### `GET /mc/v1/jobs/{handle}`
Returns authoritative execution status: `status` (`pending` | `running` | `completed` | `failed` | `cancelled`), execution timestamps, and nullable reported cost (`reported_cost_usd: Option<f64>`).

### `GET /mc/v1/jobs/{handle}/result`
Downloads raw output PNG bytes (RGB8, exact input dimensions). Validated by response headers: `X-MC-Result-Digest` (SHA256 of payload) and `X-MC-Reported-Cost-Usd`.

### `POST /mc/v1/jobs/{handle}/cancel`
Requests cancellation. Response returns `{ "handle": "...", "status": "cancel_requested", "acknowledged": true }`. Does NOT immediately transition to terminal `cancelled`; client must reconcile terminal state via subsequent status inquiry.

### `POST /mc/v1/warmup`
Separate, explicit, potentially billable worker readiness probe. Triggers GPU allocation and model weight loading to memory.

---

## 4. Client Lifecycle & Dispatch State Machine

```text
[Intent Persisted] -> [HTTP POST /jobs]
       |                      |
       | (Network Error)      +--> 202 Accepted -> [Handle Persisted] -> [Poll Status]
       v                                                                      |
[submission_unknown]                                                          +--> completed -> [Validate Result] -> [Durable Cache] -> [Project Commit]
(No auto-retry)                                                               +--> failed    -> [Log Error]       -> [Terminal Failed]
                                                                              +--> cancel    -> [cancel_requested]-> [Reconcile Cancel]
```

### Invariants:
1. **Intent-Before-Dispatch:** Client logs an intent record containing `{job_id, attempt_id, request_digest, grant_id}` before initiating network transmission.
2. **Ambiguous Transmission (`submission_unknown`):** If a network timeout, disconnect, or ambiguous 5xx/429 occurs prior to receiving a 202 handle, the attempt transitions locally to `submission_unknown`. Automatic retry or provider failover is strictly forbidden.
3. **Handle Durability:** The received `handle` must be durably committed before status polling starts.
4. **Result Validation & Durable Caching:** Before committing to the project, the downloaded raster is validated: encoded and decoded bounds are enforced before allocation/decode, followed by exact byte digest verification (`X-MC-Result-Digest`), RGB8 color mode, and post-decode pixel geometry identical to input bounds. Normal completed results MUST be validated and durably cached in the journal before project commit.
5. **Stale/Detached Results:** If a region is deleted or modified locally while a remote job is in flight, the completed patch is safely cached in the journal but detached from project commit.
6. **No Blind Retries:** Re-issuing `POST /jobs` is permitted only upon authoritative pre-enqueue rejection (confirmed structured error body from endpoint) or confirmed pre-transmission socket failure within the active grant. A raw HTTP 400 alone does NOT prove pre-enqueue rejection. Repeated reads of `/jobs/{handle}` or `/result` for an existing handle may retry with exponential backoff.
