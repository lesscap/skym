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
| `skym agent` (planned) | Run in the foreground, collecting and reporting every interval; managed by systemd | yes |
| `skym doctor` (planned) | Check the installation: configuration, Docker socket access, server reachability, token validity | yes |

### `status`

```
host i · 1 critical · 1 warn
CRIT  dify/weaviate     CRASH_LOOP       12 restarts in the last hour
WARN  /data             DISK_FILLING     87% used
```

With `--json`, the output has the same shape as `GET /api/hosts/{host}` in the [query API](api.md), so an agent reads a host the same way locally and remotely.

Findings that need history only the server has (lost heartbeats, disk growth projection, endpoint and TLS probes) do not appear locally. Crash restarts and container OOM kills come from Docker's event buffer, which keeps only the last 256 events, so locally they are a lower bound; a container that is crash-looping right now still shows as `WORKLOAD_DOWN` (restarting). Host-level OOM kills need a previous reading, so they are left to `skym agent` (planned).

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

Planned with `skym agent`: `server`, `token_file` (`/etc/skym/token`, mode 0600, kept out of the configuration) and the state directory `/var/lib/skym/` (undelivered reports, log cursors). Public IP addresses are never reported.

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
| OOM kills | Docker `oom` events and `State.OOMKilled` from inspect for containers; for the host, the `oom_kill` counter in `/proc/vmstat` (planned, needs `skym agent`) | `docker` group |
| Datastores | A protocol handshake from the host to the container's address (Postgres SSLRequest, Redis `PING`, MySQL greeting), no credentials. Postgres replication lag is queried with credentials read locally from inspect | `docker` group |
| systemd units | Unit state; listening sockets from `/proc/net/tcp*` | nothing |

`skym` never runs `docker exec`. It does not execute anything inside containers.

Every Docker call has a deadline. Reading logs tries `tail` first and falls back to `since` alone: some Docker versions (seen on 24.0) never return a `tail` read of certain json-file logs, while `since` alone scans large logs from the start.

## Running (planned)

`skym agent` will run as a dedicated `skym` user under systemd:

```ini
[Service]
User=skym
SupplementaryGroups=docker
ExecStart=/usr/local/bin/skym agent
ProtectSystem=strict
ReadWritePaths=/var/lib/skym
NoNewPrivileges=true
PrivateTmp=true
Restart=always
```

Releases will be static (musl) binaries for amd64 and arm64 with SHA-256 checksums, published on GitHub Releases. Until then, build from source with `cargo build --release`.
