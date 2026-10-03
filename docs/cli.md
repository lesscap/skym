# skym CLI

`skym` is the binary installed on every monitored host. It serves three readers: a person debugging on the host, an AI agent running on the host, and the operator installing it.

## Commands

| Command | Purpose | Network |
| --- | --- | --- |
| `skym status [--json]` | What is wrong on this host right now: collect once, run `judge`, print findings | no |
| `skym exceptions [--workload <project/service>] [--since 1h] [--json]` | Exception groups on this host, in full detail | no |
| `skym report --dry-run` | Print the exact report that would be sent | no |
| `skym schema <status\|exceptions\|report>` | Print the JSON Schema of a command's `--json` output | no |
| `skym --version` | Print the version | no |
| `skym agent` | Run in the foreground, collecting and reporting every interval; managed by systemd | yes |
| `skym doctor` | Check the installation: configuration, token file, state directory, collection, server reachability, and one real report without exception groups, so a running agent's counts are not doubled (exit code 1 if any check fails) | yes |

### `status`

```
host i · 1 critical · 1 warn
CRIT  dify/weaviate     CRASH_LOOP       12 restarts in the last hour
WARN  /data             DISK_FILLING     87% used
```

With `--json`, the output has the same shape as `GET /api/hosts/{host}` in the [query API](api.md), so an agent reads a host the same way locally and remotely.

Findings that need history only the server has (lost heartbeats, disk growth projection, endpoint and TLS probes) do not appear locally. Crash restarts and container OOM kills come from Docker's event buffer, which keeps only the last 256 events, so locally they are a lower bound; a container that is crash-looping right now still shows as `WORKLOAD_DOWN` (restarting). Host-level OOM kills and systemd restarts need a previous reading, so only `skym agent` reports them.

Disk usage follows `df`: blocks reserved for root count neither as used nor as available.

Exit codes follow the Nagios convention, so scripts and existing monitoring tools can use them directly:

| Code | Meaning |
| --- | --- |
| 0 | ok |
| 1 | at least one warn |
| 2 | at least one critical |
| 3 | nothing could be collected |

When some sources fail (for example, no access to the Docker socket), the errors go to stderr and the exit code is at least 1: a gap in what was observed never reads as healthy.

### `exceptions`

The server cannot ask a host for more data, so details that are not reported are available here instead. Nothing printed by this command leaves the host, so it is not limited by report size: business groups include samples and stack traces are not truncated. Values that look like secrets are still redacted, since terminal output ends up in scrollback, tickets and chat.

Each command reads at most the last 20,000 log lines per container; on a very busy container, older lines in the window are not counted.

### `report --dry-run`

Shows exactly what leaves the host. Host owners can audit it before and after installation.

## Configuration

`/etc/skym/config.toml`, or the path given with `--config`. A missing default file means the defaults below.

```toml
interval = "60s"

[docker]
enabled = true                       # false if the host owner does not grant Docker socket access
socket = "/var/run/docker.sock"
exclude = ["noisy-container"]

[[systemd]]                          # one entry per service that does not run in Docker
unit = "xray"
ports = [443]                        # optional: ports that must be listening
```

`skym agent` and `skym doctor` also need:

```toml
server = "https://skym.example.com"
token_file = "/etc/skym/token"       # mode 0600, kept out of the configuration
state_dir = "/var/lib/skym"          # undelivered reports and log cursors
```

`HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY` are honored. Public IP addresses are never reported.

### `agent`

Each interval (the first one at start; a pass that overruns skips the missed ones):

1. Collect. Container events are read from where the last successful query ended, reaching 15 s further back for context (a `die` right after a `kill` is a requested stop). Crash restarts are remembered for an hour, so the count does not depend on Docker's 256-event buffer.
2. Store the report in `state_dir/outbox`, then move the log cursors on: a crash in between reads the same lines again rather than losing them. If the report cannot be stored (a full disk), it is sent right away instead, so the host does not go silent.
3. Send stored reports oldest first. A 2xx or a rejection (400, 413, 422: it will never be accepted) removes the report; anything else keeps it and waits for the next pass. A stored report that cannot be read is dropped. Reports older than 24 hours, and beyond 1440 stored, are dropped.

Lower bounds and limits:

- Host OOM kills: the kernel's `oom_kill` counter, minus the container OOM kills seen in the same pass, gives at most one host-level OOM event per pass.
- systemd restarts are counted from `NRestarts` between passes; restarts before `skym` started are unknown.
- Logs are read from each workload's cursor (a workload that never logged: from the previous pass), but never more than an hour back: lines written while `skym` was down for longer are not counted. A line still missing its newline is left for the next pass.
- What `skym` remembers between passes (restarts, counters) is lost when it restarts; the next pass starts from Docker's event buffer again.

The host configuration holds no host name and no customer: the server derives both from the token. Endpoints and certificates are probed by the server and configured there.

## Workload sources

| Source | How workloads are found | Workload key |
| --- | --- | --- |
| `docker` | Discovered: containers with a restart policy or belonging to a compose service | `(host, compose project or "-", service or container name)`; replica `n > 1` of a scaled service is `<service>#<n>` |
| `systemd` | Declared in `[[systemd]]` | `(host, "_systemd", unit)` |

Compose project names cannot be `-` or start with `_`, so keys never collide. Both sources produce the same workload model; judgement, incidents and the API do not depend on the source. A systemd unit that is active but not listening on a declared port is `WORKLOAD_DOWN`.

## Permissions

Every check is read-only.

| Data | How | Needs |
| --- | --- | --- |
| Containers | Docker API: list, inspect, events, logs | `docker` group |
| Memory | cgroup files | nothing |
| OOM kills | Docker `oom` events and `State.OOMKilled` from inspect for containers; for the host, the `oom_kill` counter in `/proc/vmstat` | `docker` group |
| Datastores | A protocol handshake from the host to the container's address (Postgres SSLRequest, Redis `PING`, MySQL greeting), no credentials. Postgres replication lag is queried with credentials read locally from inspect | `docker` group |
| systemd units | Unit state; listening sockets from `/proc/net/tcp*` | nothing |

`skym` never runs `docker exec`. It does not execute anything inside containers.

Every Docker call has a deadline. Reading logs tries `tail` first and falls back to `since` alone: some Docker versions (seen on 24.0) never return a `tail` read of certain json-file logs, while `since` alone scans large logs from the start.

## Running

`skym agent` runs as a dedicated `skym` user under systemd, with the unit in [`deploy/skym.service`](../deploy/skym.service). See [installation](install.md).

Releases will be static (musl) binaries for amd64 and arm64 with SHA-256 checksums, published on GitHub Releases. Until then, build from source.
