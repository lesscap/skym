# Domain model

The model is derived from the questions a reader — an AI agent or a person — asks:

| Question | Answered by |
| --- | --- |
| Where is something wrong right now? | Open incidents rolled up by customer and host |
| What is going on with this host? | Host facts and state, its workloads, its open incidents |
| What is going on with this application? | Workload state, exception groups, recent deployments |
| When did it start, and what else happened? | Incidents and events on a timeline |

## Three shapes of data over time

| Shape | Meaning | Examples |
| --- | --- | --- |
| **State** | What it is like now | Host reachable, container running, disk 73% used |
| **Event** | Something happened at a point in time | New image deployed, container restarted, OOM kill, host rebooted |
| **Incident** | A problem that lasts from open to resolved | Certificate expiring, endpoint returning 5xx, heartbeat lost |

Events never alert by themselves; they explain incidents. Incidents are what a reader acts on.

## Entities

```rust
struct Customer { id: CustomerId, name: String }

struct Host {
    id: HostId,
    customer: CustomerId,
    facts: HostFacts,
    state: HostState,
    last_seen: Timestamp,
}

struct Workload {
    key: WorkloadKey,
    facts: WorkloadFacts,        // kind: App | Datastore | Proxy, plus the datastore engine
    state: WorkloadState,
}

struct WorkloadKey {
    host: HostId,
    project: String,             // compose project; "-" outside compose; "_systemd" for systemd units
    service: String,             // compose service, container name, or systemd unit
}

struct Endpoint {
    url: Url,
    workload: Option<WorkloadKey>,
}
```

```text
Customer 1─* Host 1─* Workload 1─* ExceptionGroup
Endpoint *─? Workload
Incident.subject ∈ Host ∪ Workload ∪ Mount ∪ Endpoint
Event.subject    ∈ Host ∪ Workload
```

### Subjects

Incidents and events point at a subject, written everywhere (API, configuration, storage) as one string:

```text
host:<host>                          host:i
workload:<host>/<project>/<service>  workload:i/dify/weaviate
mount:<host>:<path>                  mount:i:/data
endpoint:<url>                       endpoint:https://vocra.io
```

### Workload identity

A workload is the application a reader cares about, not a container instance. Container IDs change on every recreation; `(host, project, service)` does not. This keeps incidents, exceptions and deployments of the same application connected across redeploys.

Scaled compose services keep one workload per replica: the first replica uses the service name, replica `n > 1` uses `<service>#<n>` (from `com.docker.compose.container-number`). Without scaling, keys are plain service names.

```text
valid(key) ⇔ host ≠ "" ∧ host has no "/" or ":" ∧ project ≠ "" ∧ project has no "/" ∧ service ≠ ""
```

Every key in a report must be valid and appear once among its workloads; the server rejects reports that break this.

A container is a workload only if it is meant to stay up:

```text
is_workload(c) ⇔ c.restart_policy ∈ {always, unless-stopped, on-failure}
               ∨ (c belongs to a compose service ∧ c is not a `compose run` one-off)

several containers with one key (old and new during a redeploy) ⇒ keep the running one, then the newest

workload absent from reports for 7 days ⇒ archived
```

Short-lived containers (jobs, CI, `--rm`) are only counted in host state. They are not tracked individually and their logs are not read.

## Facts and state

Each entity carries two groups of attributes.

| | Facts | State |
| --- | --- | --- |
| Changes | Rarely | Every interval |
| Host | hostname, OS, kernel, architecture, CPU count, memory total, mounts and file system types, Docker version, `skym` version, boot time | load, memory used, size, used space and inodes per mount, counts of short-lived containers |
| Workload | kind and datastore engine, image and image ID, created time, restart policy, port mappings, memory limit, whitelisted labels, healthcheck defined, log driver options | run state, crash restart times within the last hour, healthcheck result, memory used, datastore probe result |

A change in facts produces an event:

```text
workload.facts.image or image_digest changed       ⇒ Deployed { from, to }   (12-character image IDs when only the ID changed)
workload.facts (other fields both sides have) changed,
  both reported by the same skym version            ⇒ ConfigChanged
host.facts.boot_time          changed ⇒ HostRebooted
host.facts.kernel             changed ⇒ KernelChanged
```

## Events

```rust
enum EventKind {
    Deployed { from: String, to: String },
    ConfigChanged,
    Restarted,
    OomKilled,
    HostRebooted,
    KernelChanged,
}
```

Events derived from facts are produced by the server by comparing reports. Events only visible on the host (OOM kills) are reported by `skym`.

## Incidents

```rust
struct Incident {
    subject: Subject,
    code: IncidentCode,
    severity: Severity,          // Warn | Critical
    opened_at: Timestamp,
    last_seen: Timestamp,
    resolved_at: Option<Timestamp>,
    detail: String,
}
```

`(subject, code)` identifies an incident: while it is open, repeated detections update `last_seen` instead of opening a new one.

```text
status(subject) = max(severity of open incidents on subject and its children), ok if none

now − host.last_seen > 3 × report_interval ⇒ open Incident { subject: host, code: HEARTBEAT_LOST, severity: Critical }
```

### Incident codes (first version)

| Code | Subject | Source |
| --- | --- | --- |
| `HEARTBEAT_LOST` | Host | No report within the timeout |
| `WORKLOAD_DOWN` | Workload | Should be running but is not; `Exited (0)` is not a failure |
| `WORKLOAD_UNHEALTHY` | Workload | Docker healthcheck reports unhealthy |
| `CRASH_LOOP` | Workload | Restart count grows faster than a threshold |
| `OOM_KILLED` | Host or Workload | Kernel OOM kill |
| `DISK_FILLING` | Mount | A mount is nearly full, or projected to fill up soon |
| `LOG_UNBOUNDED` | Workload | `json-file` log driver without a size limit |
| `DATASTORE_UNREACHABLE` | Workload (Datastore) | A discovered database does not answer a local probe |
| `REPLICATION_LAG` | Workload (Datastore) | Postgres standby lags behind |
| `ENDPOINT_DOWN` | Endpoint | 5xx or timeout from an external probe |
| `CERT_EXPIRING` | Endpoint | TLS certificate expires soon (read from the probe's handshake) |
| `APP_EXCEPTIONS` | Workload | Application-class exception groups above a threshold |

Thresholds, open/resolve rules and muting are defined in [judgement rules](judgement.md).

## Exception groups

```rust
struct ExceptionGroup {
    workload: WorkloadKey,
    class: ExceptionClass,       // Application | Business
    component: String,
    code: String,
    count: u32,
    final_count: u32,            // failures that will not be retried
    first_seen: Timestamp,
    last_seen: Timestamp,
    biz_keys: Vec<String>,       // up to 5 most recent distinct values
    sample: Option<ExceptionSample>, // application class only; truncated, redacted
}
```

```text
class = Application ∧ over threshold ⇒ Incident APP_EXCEPTIONS
class = Business                     ⇒ queryable only, never an incident
```

The line format and grouping rules are defined in the [exception protocol](exception-protocol.md).
