# skym API reference

Read this when [SKILL.md](SKILL.md) is not enough: which parameters an endpoint takes, what a field means, what an incident code is about. Every call takes `Authorization: Bearer $SKYM_TOKEN`; `GET /api` lists the same endpoints.

## Endpoints

| Endpoint | Parameters | Returns |
|---|---|---|
| `/api/overview` | | `status`, `problems` (open, unmuted incidents, worst and oldest first), `hosts` (each with its own `incidents` and `info_count`), `muted_count` |
| `/api/apps` | | every application, most urgent first (no filter: select by the host in `key`, or by `tags`) |
| `/api/apps/{host}/{project}` | | `{ app, workloads }`; a lone container or systemd unit at `/api/apps/{host}/{project}/{service}` |
| `/api/hosts` | | every host with its system, resources and disks |
| `/api/hosts/{host}` | | the host's `facts`, `state`, `workloads`, `apps`, open `incidents` and collector `errors` |
| `/api/hosts/{host}/workloads/{project}/{service}` | | one workload's `facts`, `state`, `incidents`, recent `exceptions` and `events` |
| `/api/timeline` | `host` (required), `workload` (`project/service`), `since` (default `6h`), `limit` | `entries`: incident changes and events, by time, with `truncated` |
| `/api/incidents` | `status` (`open` default, or `resolved`), `host`, `code`, `since` (resolved only, default `24h`), `include_muted` (default false), `limit` (default 100, at most 1000) | incidents, with `truncated` |
| `/api/exceptions` | `host` (required), `workload`, `class` (`application` or `business`), `since` (default `1h`), `limit` | exception groups, with `truncated` |

`since` is a duration (`15m`, `6h`, `7d`) or an RFC 3339 time. `{project}` is `-` for a lone container, `_systemd` for a systemd unit. Errors answer `{ "error": "not_found" | "bad_request" | "unauthorized" | "internal", "message": … }`.

## Fields

**Incident**: `subject` (`host:<h>`, `workload:<h>/<project>/<service>`, `mount:<h>:<path>`, `endpoint:<url>`, `app:<key>`), `code`, `severity` (`critical`, `warn`, `info`), `detail`, `opened_at`, `open_for`, `since` (when it really began, if known), `resolved_at` (`open_for` is then null), `muted` and `mute_reason`, `app`, `observed_since`, `links`; a workload's carries `workload` (below).

**Workload** (in apps, host views and incidents): `key`, `kind` (`app`, `datastore`, `proxy`), `status`, `run` (`running`, `restarting`, `exited`, `inactive`…), `exit_code`, `state_since`, `image`, `restart_policy`, `ports`, `memory_used_bytes` and `memory_limit_bytes`, `cpu_cores` (CPUs kept busy), `restarts_last_hour`, `health_output` (the failing check's output).

**Workload view** adds `facts`: `datastore` (`postgres`, `redis`, `mysql`), `image_digest`, `created`, `labels`, `healthcheck`, `log_driver`, `log_max_size`; and `state`: `health`, `restarts` (times), `missing_ports` (declared but not listening), `oom_killed`, `datastore` (`reachable`, `detail`, `replication_lag_s`: measured only on a Postgres standby whose credentials the host's agent can read; null with `detail: ok` means not measured, and `lag unknown: …` that the query failed), `health_failing_streak`.

**Application** (every probed URL is in some application's `endpoints`): `key` (`<host>/<project>[/<service>]`; `external/<name>` runs where skym does not watch and is only probed), `name`, `env`, `note`, `configured` (false: discovered, not described in the configuration), `status`, `services` and `running`, `last_deployed`, `deploys` (the latest 10: `ts`, `service`, `from`, `to` image; older ones in the timeline), `exceptions_1h`, `tags`, `workloads`, `endpoints`.

**Endpoint** (a probed URL): `url`, `status`, `http_status`, `latency_ms`, `cert_expires_at`, `last_probe_ago`, `incidents`. Probed from the server's host once a report interval.

**Host** (overview, `/api/hosts`): `last_report_ago`, `load_1m`, `cpu_percent`, `iowait_percent`, `steal_percent`, `memory_used_bytes` and `memory_total_bytes`, `net_rx_bytes_per_s` and `net_tx_bytes_per_s`, `other_cpu_cores` and `other_memory_bytes` (beyond its workloads; memory is a lower bound), `disks` (`path`, `used_percent`, `free_bytes`, `total_bytes`, `inodes_percent`, `fs_type`, `filling`), `os`, `kernel`, `arch`, `cpu_count`, `boot_time`, `docker_version`, `agent_version`, `ip`, `tags`, `apps` and `apps_in_trouble`. The host view has its resources under `state` (disks as `state.mounts`, in bytes and inodes) and adds `load_5m`, `load_15m`, `transient_containers` (short-lived containers, running and exited), `public_ip` when reported, and `errors`: what the agent could not read, so what skym may not know about that host.

**Timeline entry**: `ts`, `subject`, `type`: `incident_opened`, `incident_reopened`, `incident_resolved`, `severity_changed`, or `event` with `event.type`:

| Event | Meaning |
|---|---|
| `deployed` | a new image (`from` → `to`) |
| `config_changed` | the workload's configuration changed, same image |
| `restarted` | the container restarted |
| `oom_killed` | the kernel killed it for memory |
| `host_rebooted` | the host booted again |
| `kernel_changed` | a new kernel after a reboot |

**Exception group**: `workload`, `class` (`application` is a fault; `business` an expected outcome such as a declined payment, never an incident), `component` (`_stderr` for unmarked error lines, whose `code` is `_UNSTRUCTURED:<hash>`), `code`, `count`, `final_count` (failed for good, after retries), `first_seen`, `last_seen`, `biz_keys` (the business objects affected), `sample` (`message`, `exception_type`, `stacktrace`).

## Incident codes

| Code | Opens when | Severity | Check next |
|---|---|---|---|
| `HEARTBEAT_LOST` | no report for 3 minutes | critical | the host or its network; "all hosts silent": the server side |
| `WORKLOAD_DOWN` | not running in 2 reports (`Exited (0)` aside) | critical | `exit_code`, OOM, restart policy, timeline for a deploy |
| `WORKLOAD_UNHEALTHY` | running but unhealthy in 2 reports | critical, warn after 24 h | `health_output` |
| `HEALTHCHECK_BROKEN` | the image's healthcheck cannot even start (a missing `wget`…) | info | health unknown: judge by its URLs; fix the image's check |
| `CRASH_LOOP` | ≥ 3 crash restarts in an hour | warn, critical at ≥ 10 | `exit_code`, exceptions, a recent deploy |
| `OOM_KILLED` | an OOM kill in the last hour | warn, critical at ≥ 3 | `memory_used_bytes` against `memory_limit_bytes` |
| `DISK_FILLING` | ≥ 85% space or inodes, or full within 7 days | critical at ≥ 92% or within 24 h | which disk, `free_bytes`; the projection uses the last 6 h |
| `LOG_UNBOUNDED` | containers log without `max-size` (one per host, names them) | info | hygiene: set log limits |
| `DATASTORE_UNREACHABLE` | the local probe fails twice | critical | the workload view's `state.datastore.detail` |
| `REPLICATION_LAG` | lag > 30 s | critical at > 5 min | `state.datastore.replication_lag_s` |
| `ENDPOINT_DOWN` | 2 probes without an answer or with a bad status | critical for no answer or 5xx, else warn | the app's services: running means a proxy, DNS or certificate between |
| `CERT_EXPIRING` | the served certificate expires within 14 days | critical within 7 days | `cert_expires_at` |
| `APP_MISSING` | a configured application has no container on its host | critical in `prod`, else warn | removed on purpose, or a failed deploy |
| `APP_EXCEPTIONS` | in 15 min: ≥ 5 failed for good, ≥ 50 retries, or ≥ 10 `_stderr` lines | critical at ≥ 20 failed for good | `/api/exceptions` for the groups and a sample |

An incident resolves after its condition clears (most need 2 good reports); one that recurs within 30 minutes reopens with the same `opened_at`. Muted incidents (server configuration, with a `reason` and maybe an `until`) never count towards a status and leave the overview; `/api/incidents?include_muted=true` lists them.
