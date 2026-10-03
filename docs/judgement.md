# Judgement rules

How reports become findings and findings become incidents. Incident codes and entities are defined in the [domain model](domain-model.md).

## Two layers

```rust
// crates/core — pure, no I/O, shared by `skym` and `skym-server`
fn judge(input: &JudgeInput) -> Vec<Finding>

struct JudgeInput<'a> {
    report: &'a Report,                                // its `ts` is the reference time
    facts: &'a BTreeMap<WorkloadKey, WorkloadFacts>,   // for workloads whose report omits facts
    recent: Recent<'a>,                                // OOM kills (1 h), exception groups (15 min)
    open: &'a BTreeSet<(Subject, IncidentCode)>,       // selects resolve thresholds
}

struct Finding {
    subject: Subject,
    code: IncidentCode,
    severity: Severity,
    detail: String,
}

// crates/server — stateful, owns the incident lifecycle
fn lifecycle::next(rule, active, reopenable, finding, host, now) -> Transition   // pure
fn evaluate::apply(conn, findings, active, observed, now) -> Result<()>          // stores transitions
```

- `skym status` on a host runs `judge` only: what is wrong at this moment.
- `skym-server` adds time: opening, resolving and reopening incidents, plus findings only it can make (lost heartbeats, disk growth projection, endpoint and certificate probes).

Rules that need a short history get it as input, so `judge` stays pure: `skym` reports the times of crash restarts within the last hour as workload state, and the caller passes recent OOM kills and exception groups. Locally, `skym status` collects them in the same pass; on the server, they come from the store.

## Incident lifecycle

```text
open     ⇐ the rule matches in N consecutive evaluations
resolve  ⇐ the rule does not match in M consecutive evaluations
reopen   ⇐ the rule matches N times again within 30 minutes after resolve → the same incident is reopened, not a new one
severity ⇐ follows the latest finding (info ↔ warn ↔ critical); the peak severity is kept

numeric thresholds use hysteresis: open at X, resolve below X − δ
```

N and M count reports, not minutes: the times below assume the default 60-second report interval.

## Default rules

| Code | Opens when | Severity | Resolves when |
| --- | --- | --- | --- |
| `HEARTBEAT_LOST` | No report for 3 intervals (3 minutes by default) | critical | A report arrives |
| `WORKLOAD_DOWN` | Not running in 2 consecutive reports; `Exited (0)` does not count | critical | Running in 2 consecutive reports |
| `WORKLOAD_UNHEALTHY` | Running and unhealthy in 2 consecutive reports (a stopped workload is `WORKLOAD_DOWN`) | critical; warn after 24 hours open | Healthy in 2 consecutive reports |
| `CRASH_LOOP` | ≥ 3 crash restarts within an hour | warn; critical at ≥ 10 | No restart for 30 minutes |
| `OOM_KILLED` | An OOM kill within the last hour | warn; critical at ≥ 3 in an hour | No OOM kill for an hour |
| `DISK_FILLING` | Used ≥ 85% (space or inodes), or projected full within 7 days | critical at ≥ 92% or projected full within 24 hours | Used below threshold − 3 points and projection beyond 7 days |
| `LOG_UNBOUNDED` | Any container uses the `json-file` log driver without `max-size` (the `local` driver rotates by default). One incident per host, naming the containers | info | Every container has a size limit |
| `DATASTORE_UNREACHABLE` | Local probe fails twice in a row | critical | Probe succeeds twice in a row |
| `REPLICATION_LAG` | Lag > 30 seconds | critical at > 5 minutes | Lag < 10 seconds for 5 minutes |
| `ENDPOINT_DOWN` | 2 consecutive probes get no answer (connection, TLS, 10 s timeout) or a status other than expected: by default 400 or above, else one listed in `expect` | critical for no answer or 5xx; warn otherwise | 2 consecutive good probes |
| `CERT_EXPIRING` | The certificate of an https answer expires within 14 days | critical within 7 days or expired | A certificate expiring later is served |
| `APP_EXCEPTIONS` | Within 15 minutes: `final_count` ≥ 5, or non-final count ≥ 50, or `_stderr` count ≥ 10 | critical at `final_count` ≥ 20 | Below every threshold for 30 minutes |

Notes:

- **No memory percentage rule.** Linux uses free memory as cache, so high usage alone is not a problem; OOM kills are.
- **Severities**: `critical` and `warn` set a host's status; `info` is hygiene (an open incident that does not make a host unwell).
- **Details say since when and why.** `WORKLOAD_DOWN` names the exit code, how long the workload has been down (it may predate skym), an OOM kill and a missing restart policy; `WORKLOAD_UNHEALTHY` the number of failed checks and the last check's output; `DATASTORE_UNREACHABLE` the address probed.
- **A datastore** is a container whose image is a known engine and which runs that engine's server (no command, only flags, or the server binary): a backup job on a `postgres` image is an application.
- **`WORKLOAD_UNHEALTHY` decays to warn after 24 hours.** A container that stays unhealthy without business impact would otherwise hold a critical status forever and hide new problems. Other codes keep their severity while open.
- **Disk projection** uses a least-squares fit over the last 6 hours of used space. It needs at least 30 samples spanning 5 hours; with less, only the percentage applies.
- `business` exception groups never open incidents; see the [exception protocol](exception-protocol.md). Neither do `_PROTOCOL_ERROR` groups: they point at the application's logging, not at a failure in production. `_OVERFLOW` groups count like any other application failure.
- **Probes** run on `skym-server`'s host, once per report interval: a `GET` with the endpoint's configured headers, connecting directly (proxy settings are ignored). Redirects are followed within the same origin (scheme, host and port) only, so a probe token goes nowhere else; a redirect elsewhere, http to https included, counts as the answer: configure the final URL. Each probe opens a fresh connection, so DNS, connecting and the current certificate are part of it. An endpoint not probed for three intervals shows as `unknown`. A URL served from the server's own host is not seen from outside. A certificate is judged only when the endpoint answered: a failed handshake (an expired certificate, for one) is `ENDPOINT_DOWN`.
- All values above are defaults. Rule thresholds are defined in `crates/core`; heartbeat and disk projection, which only the server can judge, in `crates/server`.

## Muting

Some conditions are known and accepted: a container that is always unhealthy, a disk that normally runs at 90%. They can be muted in the server configuration:

```toml
[[mute]]
subject = "workload:web-1/legacy/worker"
code = "WORKLOAD_UNHEALTHY"
reason = "known issue, no business impact"

[[mute]]
subject = "mount:db-1:/data"
code = "DISK_FILLING"
until = "2026-12-31T00:00:00Z"
```

`subject` is a subject string as the API returns it (`host:web-1`, `workload:web-1/project/service`, `mount:web-1:/path`); `until` is an RFC 3339 time. Host-wide codes such as `LOG_UNBOUNDED` are muted on `host:web-1`, which also covers containers added later.

```text
muted(incident) ⇔ ∃ m ∈ mute: m.subject = incident.subject
                             ∧ m.code = incident.code
                             ∧ (m.until = none ∨ now < m.until)

muted incidents are still recorded and returned by the API with muted = true
status(subject) ignores muted incidents
```

Muting hides nothing: an agent or a person can always list muted incidents and their reasons.
