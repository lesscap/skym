# skym

Agent-first monitoring for a handful of Linux hosts and the containers running on them.

A small `skym` binary runs on every host. It checks the host, its containers and the applications inside them, then pushes a compact report to `skym-server` over outbound HTTPS. The server keeps the latest state, turns changes into events and problems into incidents, and exposes everything through a JSON API that an AI agent can read directly. A web UI comes later and is only another view over the same API.

What makes it different:

- **Built for agents first.** Every command and endpoint returns stable, self-describing JSON with judgements already made (`incidents[]`, `severity`, `open_for`), so an agent can reason about causes instead of re-deriving thresholds.
- **Safe to run on machines you don't own.** The host only makes outbound requests, the server never sends commands back, and credentials never leave the host.
- **Application exceptions with business meaning.** Applications write one structured JSON line to stdout/stderr; skym groups failures by component and code, separates business failures from system failures, and links them to deployments and restarts on the same host.

## Documentation

- [Architecture](docs/architecture.md)
- [Domain model](docs/domain-model.md)
- [Report protocol](docs/report-protocol.md)
- [Exception protocol](docs/exception-protocol.md)
- [Judgement rules](docs/judgement.md)
- [Query API](docs/api.md)
- [skym CLI](docs/cli.md)

## Status

Under development; not released yet. Build from source with `cargo build --release`.

| Part | State |
| --- | --- |
| `skym status`, `exceptions`, `report --dry-run`, `schema` | Working: host, Docker, systemd, datastore and container log checks |
| `skym-server` | Working: report ingest, incidents, heartbeat loss, disk projection, query API |
| `skym agent` (collect and push on an interval, buffer and replay), `skym doctor` | Next |
| Endpoint and TLS probes, releases and install, the agent skill, notifications, web UI | Later |

## License

[MIT](LICENSE)
