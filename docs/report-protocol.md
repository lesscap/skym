# Report protocol

`skym agent` (planned) sends one message type, `Report`, on a fixed interval. Each report is also the host's heartbeat.

## Transport

```http
POST /api/report
Authorization: Bearer <host token>
Content-Type: application/json
Content-Encoding: gzip
```

- The server identifies the host from the token. The `host` field in the body is informational and ignored for identity.
- A report with an invalid or duplicate workload key is rejected with `400` (see [workload identity](domain-model.md#workload-identity)).
- A report timed more than 10 minutes ahead of the server, or more than 7 days behind it, is rejected with `400` and does not count as a heartbeat: a host clock that ran ahead would otherwise make every later report look old.
- A report not newer than the host's last one (compared in whole seconds) is acknowledged and counts as a heartbeat, but changes nothing else. Replaying buffered reports is therefore safe.
- At most 2 MiB compressed and 32 MiB decompressed.
- The response only acknowledges receipt. It never carries instructions.
- Default interval: 60 seconds.

## Message

```rust
struct Report {
    host: HostId,
    ts: Timestamp,                     // collection time on the host
    host_facts_hash: String,           // 16 hex characters
    host_facts: Option<HostFacts>,
    host_state: HostState,
    workloads: Vec<WorkloadReport>,
    local_events: Vec<LocalEvent>,     // events only visible on the host, e.g. OOM kills
    exceptions: Vec<ExceptionGroup>,   // groups observed since the previous report
    errors: Vec<String>,               // sources that failed in this pass
    agent_version: Option<String>,     // the reporting skym's version
}

struct WorkloadReport {
    key: WorkloadKey,
    facts_hash: String,
    facts: Option<WorkloadFacts>,
    state: WorkloadState,
}
```

Entity fields are defined in the [domain model](domain-model.md).

When a source fails (for example, no access to the Docker socket), the report lists it in `errors`. The server then treats subjects missing from the report as unknown, not as recovered: their incidents stay as they are.

A facts hash is the first 8 bytes of SHA-256 over the facts' JSON form, written as 16 lowercase hex characters. A string keeps it exact for every JSON consumer.

`skym` does not know its host id; it fills every host field (`host`, workload keys) with its hostname, and the server replaces them all with the host id bound to the token.

## When facts are sent

Planned with `skym agent`; until then every report carries all facts. Facts change rarely, so most reports will carry only the hash and the state.

```text
send facts for entity e ⇔ hash(e.facts) ≠ hash last sent for e
                        ∨ first report after skym starts
                        ∨ first report of the current hour
```

- The hash is sent in every report, so the server can tell whether its copy is current (it stores the hash but does not use it yet).
- The hourly resend bounds recovery: if the server loses facts (store reset, failed replay, new server), it has them again within an hour without asking the host. The server never asks; see the [security model](architecture.md#security-model).
- A workload whose facts the server does not have yet is shown with its state only.

## Server processing

| Data | Stored as |
| --- | --- |
| Facts, state | Latest value per host and workload, overwritten on each report |
| Used space per mount | Time series, used to project when a mount fills up |
| Events, incidents, exception groups | Appended, queried by time |

Events derived from facts (`Deployed`, `ConfigChanged`, `HostRebooted`, `KernelChanged`) are produced by the server by comparing the incoming facts with the stored ones ([rules](domain-model.md)). Full reports are not kept.

## Buffering and replay (planned)

When a report cannot be delivered, `skym` keeps it in a local buffer (bounded by count and age, 24 hours by default) and sends buffered reports in order once the server is reachable. A report the server already has is only a heartbeat (see [transport](#transport)), so replaying a report twice is harmless.

Log reading keeps a per-workload cursor on the host, so a restart of `skym` neither repeats nor skips log lines.

## Compatibility

Hosts in the field run different `skym` versions, and the server accepts all of them. The protocol has no version number; it follows the [evolution](architecture.md#evolution) rules. Every report names its `skym` version in `agent_version` (older reports only in host facts); the server compares workload facts for `ConfigChanged` only between reports of the same version.

## Never reported

- Container environment variables
- Labels outside the whitelist (`com.docker.compose.*`)
- Public IP addresses
- Full log lines; exception samples are truncated and filtered for common secret patterns

## Size

Estimated per container: facts about 1 KB, state about 150 bytes, as JSON.

| Containers per host | Typical report (state only, gzip) | Report with all facts (gzip) |
| --- | --- | --- |
| 50 | ~1 KB | ~5 KB |
| 500 | ~8 KB | ~55 KB |

## Collection cost on the host

| Data | Source |
| --- | --- |
| Container list and run state | One `GET /containers/json` per interval |
| Workload facts | `inspect` of each container per interval (planned: only when a container ID is new) |
| Restarts | Docker's event buffer, the last 256 events |
| Memory usage | cgroup files read directly, not `docker stats` |
| Exceptions | Incremental log reads for workloads only |
