# Query API

The API `skym-server` offers to AI agents and, later, to the UI. It is read-only. Reports from hosts use a separate endpoint described in the [report protocol](report-protocol.md).

## Design

- **One call per question.** Each of the reader's questions (see the [domain model](domain-model.md)) is answered by one endpoint.
- **Bounded responses.** The overview carries status and counts, not facts. Lists have a default limit and return `truncated: true` when cut.
- **Links to drill down.** Objects carry `links` to the endpoints that explain them (`host`, `workload`, `timeline`, `exceptions`, `incidents`), so an agent rarely builds URLs itself.
- **Self-describing.** `GET /api` lists every endpoint with its purpose and parameters.
- **Two forms of time.** Absolute timestamps (`opened_at`, RFC 3339) and durations (`open_for: "3h12m"`). Durations let an agent tell new problems from chronic ones at a glance. `open_for` is how long skym has seen a problem: compare it with the host's `observed_since`. An incident's `since`, when present, is when the problem really began (a stopped workload: when it stopped), which may predate skym.
- **Read-only.** Muting lives in the server configuration. A leaked reader token can read but not change anything.

## Authentication

```http
Authorization: Bearer <reader token>
```

Reader tokens are configured on the server. Every reader token sees all customers; there are several so that each person or agent has its own, can be told apart in the server log, and can be revoked alone.

```toml
[[readers]]
name = "alice"
token_sha256 = "…"

[[readers]]
name = "ops-agent"
token_sha256 = "…"
```

`skym-server token` prints a new random token and the hash to configure. The server stores only token hashes. It logs the reader `name` and path of each request at `debug` level (`RUST_LOG=skym_server=debug`). Host tokens can only call `POST /api/report`, and reader tokens cannot report.

## Endpoints

| Endpoint | Question | Returns |
| --- | --- | --- |
| `GET /api` | How do I use this? | Endpoint descriptions |
| `GET /api/overview` | Where is something wrong right now? | Customers → hosts with status, last report age and open, unmuted incidents; unhealthy hosts first |
| `GET /api/hosts/{host}` | What is going on with this host? | Facts, state, workloads with their status, open incidents |
| `GET /api/hosts/{host}/workloads/{project}/{service}` | What is going on with this application? | Facts, state, recent exception groups, recent events |
| `GET /api/timeline` | When did it start, and what else happened? | Incident changes and events, merged and sorted by time |
| `GET /api/incidents` | Incident history | Incidents |
| `GET /api/exceptions` | Exception groups, including business failures | Exception groups |
| `GET /healthz` | Is the server up? | `ok`, no authentication |

`{project}` is `-` for containers outside compose. Compose project names cannot be `-`, so there is no collision.

### Parameters

| Endpoint | Parameters |
| --- | --- |
| `timeline` | `host` (required), `workload` (`project/service`), `since` (default `6h`), `limit` |
| `incidents` | `status` (`open` \| `resolved`, default `open`), `host`, `code`, `since`, `include_muted` (default `false`), `limit` |
| `exceptions` | `host` (required), `workload`, `class` (`application` \| `business`), `since` (default `1h`), `limit` |

`since` takes a duration (`15m`, `6h`, `7d`) or an RFC 3339 timestamp. For `incidents` it applies to resolved ones only (default `24h`); open incidents are listed whatever their age. `limit` defaults to 100, at most 1000.

### Status

`ok`, `warn`, `critical`, or `unknown` for a configured host that never reported. Only `warn` and `critical` incidents count; `info` incidents (hygiene) are listed, and counted in the overview's `info_count`. Hosts are listed by urgency: critical, then unknown, then warn, then ok. Muted incidents never count towards a status. `incidents` lists them only with `include_muted=true`; host and workload views include them with `muted: true`; the overview leaves them out and counts them in `muted_count`.

### Lists and timeline

`incidents`, `exceptions` and `timeline` return `{ "incidents" | "exceptions" | "entries": [...], "truncated": bool }`. Timeline entries are flat, with a `type`: `incident_opened`, `incident_reopened`, `incident_resolved`, `severity_changed`, or `event` with the event under `event` (it carries its own `type`, such as `deployed`).

## Overview response

```json
{
  "ts": "2026-10-01T06:30:00Z",
  "status": "critical",
  "customers": [
    {
      "id": "lesscap",
      "status": "critical",
      "hosts": [
        {
          "id": "i",
          "status": "critical",
          "last_report_ago": "40s",
          "observed_since": "2026-09-01T08:00:00Z",
          "info_count": 0,
          "incidents": [
            {
              "code": "CRASH_LOOP",
              "severity": "critical",
              "subject": "workload:i/dify/weaviate",
              "opened_at": "2026-10-01T06:05:00Z",
              "open_for": "25m",
              "detail": "12 restarts in the last hour",
              "links": {
                "workload": "/api/hosts/i/workloads/dify/weaviate",
                "timeline": "/api/timeline?host=i&workload=dify/weaviate&since=6h"
              }
            }
          ],
          "links": { "host": "/api/hosts/i" }
        },
        {
          "id": "x",
          "status": "ok",
          "last_report_ago": "12s",
          "incidents": [],
          "links": { "host": "/api/hosts/x" }
        }
      ]
    }
  ],
  "muted_count": 2
}
```

## Errors

```json
{ "error": "not_found", "message": "host 'z' is unknown" }
```

| HTTP status | `error` |
| --- | --- |
| 400 | `bad_request` |
| 401 | `unauthorized` |
| 404 | `not_found` |
| 500 | `internal` |

Two responses do not use this shape yet: unknown paths (404) and report bodies over the size limit (413) answer with plain text.

## Compatibility

The API has no version prefix. It follows the [evolution](architecture.md#evolution) rules: endpoints, fields and parameters are only added; clients ignore fields they do not know, and read enum values they do not know (a new incident code, severity or status) as `UNKNOWN` / `unknown`.
