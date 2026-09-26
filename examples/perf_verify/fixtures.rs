use std::fmt::Write as _;

// Deterministic JSON log generator. Training seeds 1-8 and
// measurement seeds 101-164 are disjoint. Fixed-size slices may end mid-record.
pub fn payload(size: usize, seed: u32) -> Vec<u8> {
    let mut out = String::new();
    let mut state = seed;
    while out.len() < size {
        json_record(&mut out, &mut state);
    }
    out.truncate(size);
    out.into_bytes()
}
fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

pub fn text(size: usize, seed: u32) -> Vec<u8> {
    const WORDS: &str = "the of and to in is was for on that with as by at from his her \
        they this have had were which their are but not one all been when there she \
        would what so if will more no out up into could them than then some other \
        time very about only upon over such said great before after little";
    let words: Vec<_> = WORDS.split_whitespace().collect();
    let mut state = seed;
    let mut out = Vec::with_capacity(size + 16);
    while out.len() < size {
        out.extend_from_slice(words[xorshift32(&mut state) as usize % words.len()].as_bytes());
        out.push(b' ');
    }
    out.truncate(size);
    out
}

pub fn binary(size: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..size).map(|_| xorshift32(&mut state) as u8).collect()
}

fn json_record(out: &mut String, state: &mut u32) {
    const LEVELS: &[&str] = &["DEBUG", "INFO", "WARN", "ERROR", "TRACE"];
    const SERVICES: &[&str] = &[
        "api-gateway",
        "auth-svc",
        "order-svc",
        "payment-svc",
        "notify-svc",
        "inventory-svc",
        "shipping-svc",
        "billing-svc",
        "search-svc",
        "user-svc",
        "session-svc",
        "analytics-svc",
        "cache-svc",
        "config-svc",
        "audit-svc",
        "rate-limiter",
    ];
    const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];
    const PATHS: &[&str] = &[
        "/v1/widgets",
        "/v1/users",
        "/v1/orders",
        "/v2/events",
        "/v1/health",
        "/v1/sessions",
        "/v1/payments",
        "/v2/search",
        "/v1/inventory",
        "/v1/shipping",
        "/v1/analytics",
        "/v2/config",
    ];
    const REGIONS: &[&str] = &[
        "us-east-1",
        "us-west-2",
        "eu-west-1",
        "ap-south-1",
        "eu-central-1",
        "ap-northeast-1",
        "sa-east-1",
        "ca-central-1",
    ];
    const STATUSES: &[u16] = &[
        200, 201, 202, 204, 301, 302, 304, 400, 401, 403, 404, 405, 409, 422, 429, 500, 502, 503,
        504,
    ];
    const MSGS: &[&str] = &[
        "request handled successfully",
        "resource created",
        "cache miss, fetched from origin",
        "rate limit approaching threshold",
        "upstream timeout, retrying",
        "authentication token refreshed",
        "database connection pool exhausted",
        "circuit breaker tripped",
        "message queued for async processing",
        "TLS handshake completed",
        "request routed to fallback backend",
        "payload validation passed",
        "idempotency key matched existing result",
        "graceful shutdown initiated",
        "health check passed all probes",
        "retry attempt succeeded after backoff",
    ];

    let trace_id = xorshift32(state);
    let span_id = xorshift32(state);
    let user_id = xorshift32(state);
    let r = xorshift32(state) as usize;
    let level = LEVELS[r % LEVELS.len()];
    let service = SERVICES[(r >> 4) % SERVICES.len()];
    let method = METHODS[(r >> 8) % METHODS.len()];
    let path = PATHS[(r >> 12) % PATHS.len()];
    let region = REGIONS[(r >> 16) % REGIONS.len()];
    let status = STATUSES[(r >> 20) % STATUSES.len()];
    let latency = (xorshift32(state) % 5000) + 1;
    let r2 = xorshift32(state) as usize;
    let msg = MSGS[r2 % MSGS.len()];
    let host_id = xorshift32(state);
    let _ = write!(
        out,
        r#"{{"ts":"2026-04-27T12:34:56.{trace_id:08x}Z","level":"{level}","service":"{service}","trace_id":"{trace_id:08x}{span_id:08x}","span_id":"{span_id:08x}","user_id":"u-{user_id:08x}","method":"{method}","path":"{path}/{trace_id:08x}","status":{status},"latency_ms":{latency},"region":"{region}","host":"{service}-{host_id:08x}.svc.cluster.local","msg":"{msg}"}}{nl}"#,
        nl = '\n',
    );
}
