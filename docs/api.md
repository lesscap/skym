# Query API

The API `skym-server` offers to AI agents and, later, to the UI. It is read-only. Reports from hosts use a separate endpoint described in the [report protocol](report-protocol.md).

## Design

- **One call per question.** Each of the reader's questions (see the [domain model](domain-model.md)) is answered by one endpoint.
- **Bounded responses.** The overview carries status and counts, not facts. Lists have a default limit and return `truncated: true` when cut.
- **Links to drill down.** Objects carry `links` to the endpoints that explain them, so an agent never builds URLs itself.
- **Self-describing.** `GET /api` lists every endpoint with its purpose and parameters.
- **Two forms of time.** Absolute timestamps (`opened_at`, RFC 3339) and durations (`open_for: "3h12m"`). Durations let an agent tell new problems from chronic ones at a glance.
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

`skym-server token` prints a new random token and the hash to configure. The server stores only token hashes and logs the reader `name` with each request. Host tokens can only call `POST /api/report`, and reader tokens cannot report.

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

`since` takes a duration (`15m`, `6h`, `7d`) or an RFC 3339 timestamp; for resolved incidents it defaults to `24h`. `limit` defaults to 100, at most 1000.

### Status

`ok`, `warn`, `critical`, or `unknown` for a configured host that never reported. Hosts are listed by urgency: critical, then unknown, then warn, then ok. Muted incidents are listed only with `include_muted=true` and never count towards a status.

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

## Compatibility

The API has no version prefix. It follows the [evolution](architecture.md#evolution) rules: endpoints, fields and parameters are only added; clients ignore fields they do not know.
