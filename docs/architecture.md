# Architecture

## Components

```
every host                                  your own machine
┌────────────────────────────┐              ┌──────────────────────────────┐
│ skym                       │   HTTPS POST │ skym-server                  │
│  skym status / check ...   │  /api/report │  ingest reports (per-host    │
│   (CLI for humans/agents)  │ ───────────▶ │   token)                     │
│  skym agent                │  every 60s   │  SQLite store                │
│   (collect + report)       │              │  heartbeat timeout detection │
└────────────────────────────┘              │  query API                   │
                                            └──────────────▲───────────────┘
                                                           │ HTTPS GET (token)
                                                   AI agent (via skill) / UI
```

| Component | Runs on | Role |
| --- | --- | --- |
| `skym` | every monitored host | One binary. As a CLI it answers "how is this host right now" locally. As `skym agent` it collects on a fixed interval and pushes a report. |
| `skym-server` | a machine you control | Receives reports, stores the latest state, derives events and incidents, detects lost heartbeats, serves the query API. |
| skill | the agent's side | A document that tells an AI agent how to call the API and read the results. |
| UI | served by `skym-server` (later) | A read-only view over the same API. It contains no logic of its own. |

## Principles

- **Agent first, UI second.** Anything the UI can show must be available as JSON from the API or the CLI. Judgement (thresholds, severity) is done once, in shared code, never in the UI or the agent prompt.
- **Push, not pull.** The server never connects to hosts. Hosts only make outbound HTTPS requests, so no inbound port, SSH key or firewall exception is needed on a monitored host, and the monitoring keeps running on each host even when the server is down.
- **Absence of signal is a signal.** Every report doubles as a heartbeat. A host that stops reporting opens a `HEARTBEAT_LOST` incident, whether the cause is the host, the network or `skym` itself. False alarms are acceptable; silent misses are not.

## Evolution

APIs and protocols have no version numbers. They evolve by accretion, so every change keeps existing producers and consumers working.

```text
producers (skym sending reports, the server answering queries): may provide more, never less
consumers (the server reading reports, agents reading the API):  may require less, never more

allowed:   new fields, new endpoints, new optional parameters, new enum values
forbidden: removing a field, renaming it, changing its meaning, unit or type
a meaning has to change ⇒ introduce a new name; keep the old one while producers still send it
consumers ignore unknown fields and map unknown enum values to Unknown instead of failing
```

In Rust: no `deny_unknown_fields`, `#[serde(default)]` on fields added later, `#[serde(other)] Unknown` on enums.

## Security model

`skym` is designed to run on machines that belong to someone else.

- **Outbound only.** The host opens no listening port for skym.
- **No command channel.** Server responses only acknowledge receipt. The server cannot make `skym` run anything, change configuration or fetch extra data. Upgrades are pulled or installed by the host owner.
- **Per-host tokens.** Each host has its own token, which can only write reports for that host. The server derives the host identity from the token and ignores any host name in the payload.
- **Credentials stay on the host.** Probes that need credentials (for example, connecting to a database found in a container) read them and run locally; only the result is reported.
- **Data minimization.** Container environment variables are never reported. Only whitelisted labels (`com.docker.compose.*`) are reported. Public IP addresses are not reported unless enabled. Exception messages are truncated and filtered for common secret patterns before leaving the host.
- **Read-only checks.** `skym` runs as a dedicated user and only reads. It never runs `docker exec` or anything else inside containers. Note that access to the Docker socket is effectively root access; host owners who do not accept that can disable container checks.
- **Single operator.** skym is run by one team for all the hosts it looks after. Hosts are grouped by customer, but customers have no access to skym. Every reader token sees everything; each person or agent gets its own token so access can be told apart and revoked.

## Failure model

| Failure | Effect |
| --- | --- |
| A host goes down or loses network | Its heartbeat stops; the server opens `HEARTBEAT_LOST`. |
| `skym` crashes on a host | Same as above. |
| `skym-server` is down | Hosts keep checking locally and buffer reports; buffered reports are replayed in order when the server is back. The server deduplicates by `(host, ts)`. |
| All hosts go silent at once | Reported as a single incident pointing at the server side or the network, not one incident per host. |
| The server host itself dies | Planned: the server pings an external dead man's switch (healthchecks.io); missing pings alert through that service. |

## Application signals

Applications report exceptions by writing one structured JSON line to stdout or stderr, marked with a `skym` field that carries the protocol version. `skym` reads container logs incrementally through the Docker API, parses marked lines, and groups them by `(workload, component, code)`. Unmarked lines on stderr are used as a fallback signal for crashes and unhandled errors.

Applications do not depend on `skym`: no SDK, no socket, no endpoint to expose. If `skym` is missing or down, the application is unaffected.

skym is not a log platform. It reports grouped exception signals, never full logs.

See the [exception protocol](exception-protocol.md).

## Repository layout

```
skym/
├── Cargo.toml          # workspace
├── crates/
│   ├── core/           # protocol types + judgement rules, no I/O
│   ├── agent/          # bin: skym (CLI + agent)
│   └── server/         # bin: skym-server (HTTP API + SQLite)
├── ui/                 # later: Svelte 5 SPA, embedded into skym-server
└── docs/
```

`core` is shared by `skym` and `skym-server`, so the report protocol and the judgement rules have exactly one definition.
