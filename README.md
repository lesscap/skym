# skym

Agent-first monitoring for a handful of Linux hosts and the containers running on them.

A small `skym` binary runs on every host. It checks the host, its containers and the applications inside them, then pushes a compact report to `skym-server` over outbound HTTPS. The server keeps the latest state, turns changes into events and problems into incidents, and exposes everything through a JSON API that an AI agent can read directly. People read the same API in a terminal with `skym-view`.

## What it watches

| Subject | Checks |
| --- | --- |
| Hosts | still reporting (heartbeat), disks filling up (with a 7-day projection), OOM kills, reboots and kernel changes |
| Containers | down, unhealthy (with the failing check's output), crash loops, OOM kills, logs without a size limit, deployments and configuration changes |
| Datastores | Postgres, MySQL and Redis containers answering a local probe; Postgres replication lag |
| systemd units | declared services running and listening on their ports |
| Applications | exceptions they write to stdout or stderr, grouped by component and code |
| Applications | every compose project (and lone container or systemd unit), described in the configuration; a listed one that disappears |
| Endpoints | configured URLs, probed by the server: answering as expected, and certificates close to expiry |

Thresholds, severities, and when an incident opens or resolves are decided by skym, not by whoever reads the result: see the [judgement rules](docs/judgement.md).

## How it fits together

```text
 every host                         a machine you control              you
┌──────────────┐  HTTPS, outbound  ┌──────────────────┐   GET   ┌──────────────────────┐
│ skym agent   │ ────────────────▶ │ skym-server      │ ◀────── │ AI agent + skill     │
│ (systemd)    │  a report a       │ SQLite, JSON API │ ◀────── │ skym-view (terminal) │
└──────────────┘  minute           └──────────────────┘         └──────────────────────┘
```

- **Push only.** Hosts open no port and the server never connects to them; a host that goes quiet is itself the alarm.
- **The server is an API.** It judges and stores; it serves no web pages.
- **One API for people and agents.** `skym-view` and an AI agent read the same endpoints and see the same judgements.

## What makes it different

- **Built for agents first.** Every command and endpoint returns stable, self-describing JSON with judgements already made (`incidents[]`, `severity`, `open_for`), so an agent can reason about causes instead of re-deriving thresholds.
- **Safe to run on machines you don't own.** The host only makes outbound requests, the server never sends commands back, and credentials never leave the host.
- **Application exceptions with business meaning.** Applications write one structured JSON line to stdout/stderr; skym groups failures by component and code, separates business failures from system failures, and links them to deployments and restarts on the same host.

## A look

`skym-view` opens on the problems tab: every open problem, named by its application (or by its host, for the host's own). New problems come first; `≥` means the problem was already there when skym started watching, so it has lasted at least that long. `⇥` moves to the applications and the hosts. Applications are grouped by environment, host or tag (`g`), each group folded to those in trouble; `/` filters by words and by `host:`, `env:`, `tag:` or `!ok`. Every application shows the CPU and memory its services use; `s` sorts by either, and a host's page names its largest users of each. Hosts show how busy their CPUs are (and iowait), their network traffic, and what they use beyond their workloads.

```text
 skym · skym.example.com   [Problems]  Apps   Hosts      ✗ 1   ! 2             updated 3s ago
┌ Problems ──────────────────────────────────────────────────────────────────────────────┐
│    NEW                                                                                 │
│ ✗  Shop               web-1    api: exited (1), for 4m                             4m  │
│    ONGOING                                                                             │
│ !  Orders DB          db-1     postgres: replication lag 45s                      ≥6h  │
│ !  host db-1          db-1     /data: full in ~6d                                  2h  │
│                                · 2 hygiene items (h)                                   │
└────────────────────────────────────────────────────────────────────────────────────────┘
 ↑↓ move  ⏎ open  ⇥ apps  / filter  h hygiene  m muted  p preview  ? help  q quit
```

The same problems, as an AI agent reads them from `GET /api/overview` (trimmed):

```json
{
  "status": "critical",
  "problems": [{
    "subject": "workload:web-1/shop/api",
    "app": "web-1/shop",
    "code": "WORKLOAD_DOWN",
    "severity": "critical",
    "detail": "exited (1), for 4m",
    "open_for": "4m",
    "links": {
      "app": "/api/apps/web-1/shop",
      "workload": "/api/hosts/web-1/workloads/shop/api",
      "timeline": "/api/timeline?host=web-1&workload=shop/api&since=6h"
    }
  }],
  "hosts": [{ "id": "web-1", "status": "critical", "last_report_ago": "12s", "apps": 4, "apps_in_trouble": 1 }]
}
```

## Quick start

From source; the [installation guide](docs/install.md) has the details.

1. **Build** static Linux binaries for the hosts, and the view for your own machine:

   ```sh
   cargo zigbuild --release -p skym-agent -p skym-server --target x86_64-unknown-linux-musl
   cargo install --path crates/view
   ```

2. **Server.** Make a token for each host and each reader, write the configuration from [`deploy/server.example.toml`](deploy/server.example.toml) (it holds only the tokens' hashes), and run it behind a reverse proxy that terminates TLS:

   ```sh
   skym-server token
   skym-server serve --config /etc/skym-server/config.toml
   ```

3. **Agent**, on each host: install `skym`, put [`deploy/config.example.toml`](deploy/config.example.toml) at `/etc/skym/config.toml` and the host's token at `/etc/skym/token`, check the setup, then start [`deploy/skym.service`](deploy/skym.service):

   ```sh
   sudo -u skym skym doctor
   systemctl enable --now skym
   ```

4. **Look.** With `SKYM_URL` and `SKYM_TOKEN` (a reader token) in `~/.config/skym/env`:

   ```sh
   skym-view
   ```

   Or give an AI agent the [skill](skill/SKILL.md) and the same two values.

## Documentation

- **Run it:** [installation](docs/install.md), [the `skym` CLI](docs/cli.md)
- **Read it:** [query API](docs/api.md), [skill for AI agents](skill/SKILL.md)
- **Report from an application:** [exception protocol](docs/exception-protocol.md)
- **How it works:** [architecture](docs/architecture.md), [domain model](docs/domain-model.md), [report protocol](docs/report-protocol.md), [judgement rules](docs/judgement.md)

## Status

Under development; no releases yet.

| Part | State |
| --- | --- |
| `skym status`, `exceptions`, `report --dry-run`, `schema` | Working: host, Docker, systemd, datastore and container log checks |
| `skym agent`, `skym doctor` | Working: reports every interval, buffers and replays while the server is away |
| `skym-server` | Working: report ingest, incidents, heartbeat loss, disk projection, query API |
| `skym-view` | Working: a terminal view of the server for people (read-only) |
| Endpoint and TLS probes | Working: configured URLs, optional probe tokens, certificate expiry |
| Releases | Later |

## License

[MIT](LICENSE)
