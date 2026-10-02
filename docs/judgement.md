# Judgement rules

How reports become findings and findings become incidents. Incident codes and entities are defined in the [domain model](domain-model.md).

## Two layers

```rust
// crates/core — pure, no I/O, shared by `skym` and `skym-server`
fn judge(report: &Report) -> Vec<Finding>

struct Finding {
    subject: Subject,
    code: IncidentCode,
    severity: Severity,
    detail: String,
}

// crates/server — stateful, owns the incident lifecycle
fn evaluate(findings: &[Finding], store: &Store, now: Timestamp) -> Vec<IncidentChange>
```

- `skym status` on a host runs `judge` only: what is wrong at this moment.
- `skym-server` adds time: opening, resolving and reopening incidents, plus findings only it can make (lost heartbeats, disk growth projection, external endpoint and TLS probes).

Rules that need a short history are computed by `skym` on the host and reported as state, so `judge` stays pure. For example, `skym` counts restarts from the Docker events stream and reports `restarts_1h`.

## Incident lifecycle

```text
open     ⇐ the rule matches in N consecutive evaluations
resolve  ⇐ the rule does not match in M consecutive evaluations
reopen   ⇐ the rule matches within 30 minutes after resolve → the same incident is reopened, not a new one
severity ⇐ follows the latest finding (warn ↔ critical); the peak severity is kept

numeric thresholds use hysteresis: open at X, resolve below X − δ
```

## Default rules

| Code | Opens when | Severity | Resolves when |
| --- | --- | --- | --- |
| `HEARTBEAT_LOST` | No report for 3 intervals (3 minutes by default) | critical | A report arrives |
| `WORKLOAD_DOWN` | Not running in 2 consecutive reports; `Exited (0)` does not count | critical | Running in 2 consecutive reports |
| `WORKLOAD_UNHEALTHY` | Unhealthy in 2 consecutive reports | critical; warn after 24 hours open | Healthy in 2 consecutive reports |
| `CRASH_LOOP` | `restarts_1h` ≥ 3 | warn; critical at ≥ 10 | No restart for 30 minutes |
| `OOM_KILLED` | An OOM kill within the last hour | warn; critical at ≥ 3 in an hour | No OOM kill for an hour |
| `DISK_FILLING` | Used ≥ 85% (space or inodes), or projected full within 7 days | critical at ≥ 92% or projected full within 24 hours | Used below threshold − 3 points and projection beyond 7 days |
| `LOG_UNBOUNDED` | Log driver has no `max-size` | warn | A size limit is configured |
| `DATASTORE_UNREACHABLE` | Local probe fails twice in a row | critical | Probe succeeds twice in a row |
| `REPLICATION_LAG` | Lag > 30 seconds | critical at > 5 minutes | Lag < 10 seconds for 5 minutes |
| `ENDPOINT_DOWN` | 2 consecutive probes return 5xx, time out (10 s) or fail to connect | critical; warn for 4xx other than 401, 403, 404 | 2 consecutive good probes |
| `CERT_EXPIRING` | Certificate expires within 14 days | critical within 7 days or expired | A certificate with a later expiry is served |
| `APP_EXCEPTIONS` | Within 15 minutes: `final_count` ≥ 5, or non-final count ≥ 50, or `_stderr` count ≥ 10 | critical at `final_count` ≥ 20 | Below every threshold for 30 minutes |

Notes:

- **No memory percentage rule.** Linux uses free memory as cache, so high usage alone is not a problem; OOM kills are.
- **`WORKLOAD_UNHEALTHY` decays to warn after 24 hours.** A container that stays unhealthy without business impact would otherwise hold a critical status forever and hide new problems. Other codes keep their severity while open.
- **Disk projection** uses a linear fit over the last 6 hours of used space. With less than 6 hours of data, only the percentage applies.
- `business` exception groups never open incidents; see the [exception protocol](exception-protocol.md).
- All values above are defaults defined in `crates/core`.

## Muting

Some conditions are known and accepted: a container that is always unhealthy, a disk that normally runs at 90%. They can be muted in the server configuration:

```yaml
mute:
  - subject: { host: x, service: legacy-worker }
    code: WORKLOAD_UNHEALTHY
    reason: known issue, no business impact
  - subject: { host: i, mount: /data }
    code: DISK_FILLING
    until: 2026-12-31
```

```text
muted(incident) ⇔ ∃ m ∈ mute: matches(m.subject, incident.subject)
                             ∧ m.code = incident.code
                             ∧ (m.until = none ∨ now < m.until)

muted incidents are still recorded and returned by the API with muted = true
status(subject) ignores muted incidents
```

Muting hides nothing: an agent or a person can always list muted incidents and their reasons.
