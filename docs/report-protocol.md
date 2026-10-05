# Report protocol

`skym agent` sends one message type, `Report`, on a fixed interval. Each report is also the host's heartbeat.

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
    containers_listed: bool,           // Docker listed and inspected every container (default false)
}

struct WorkloadReport {
    key: WorkloadKey,
    facts_hash: String,
    facts: Option<WorkloadFacts>,
    state: WorkloadState,
}
```

Entity fields are defined in the [domain model](domain-model.md).

When a source fails (for example, no access to the Docker socket), the report lists it in `errors`. The server then treats subjects missing from the report as unknown, not as recovered: their incidents stay as they are. `containers_listed` is narrower: it says every container was listed and inspected, so one missing from the report is gone. Only then does the server judge `APP_MISSING`; reports from agents older than the field read as `false`.

`host_state` carries rates measured since the previous report: `cpu_percent`, `iowait_percent` and `steal_percent` (shares of all CPUs, 0–100, from `/proc/stat`), and `net_rx_bytes_per_s` and `net_tx_bytes_per_s` (from `/proc/net/dev`, at the interfaces backed by a device, so traffic is counted once where it enters or leaves the host; traffic that stays on the host is not counted). Each is `null` on the first report after the agent starts (a reboot included), across a gap of more than ten minutes, and when the host has no such interface. The agent reads only these kernel counters for them, with no extra processes or sampling; one it cannot read is logged and left `null`, and is not a collection error.

Each running workload's `state.cpu_cores` is the CPUs it kept busy since the previous report (1.5 = one and a half), from its cgroup's CPU time: `cpu.stat` on cgroup v2, `cpuacct.usage` on v1; a systemd unit's cgroup comes from its `ControlGroup`. It is read with the host's counters, at the end of the pass, so both span the same interval. It is `null` on a first report, for a stopped workload, for a new instance (a recreated or restarted container, a restarted unit), and without CPU accounting (systemd units on cgroup v1 hosts usually). A running workload whose cgroup is not found is logged once. `memory_used_bytes` is the cgroup's usage less its inactive page cache, on v2 and v1 alike.

A facts hash is the first 8 bytes of SHA-256 over the facts' JSON form, written as 16 lowercase hex characters. A string keeps it exact for every JSON consumer.

`skym` does not know its host id; it fills every host field (`host`, workload keys) with its hostname, and the server replaces them all with the host id bound to the token.

## When facts are sent

Facts change rarely, so most reports carry only the hash and the state. (`skym report --dry-run` shows every fact.)

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

## Buffering and replay

Every report is stored on the host before it is sent (at most 1440 reports and 24 hours) and sent oldest first once the server is reachable. A report the server already has is only a heartbeat (see [transport](#transport)), so replaying a report twice is harmless. A report the server rejects as invalid (400, 413, 422) is dropped; after any other failure it is kept and retried with the next pass.

Log reading keeps a per-workload cursor on the host (the time of the last line read), so a restart of `skym` neither repeats nor skips log lines, within the last hour. See [`skym agent`](cli.md#agent).

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
