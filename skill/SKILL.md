---
name: skym
description: Check the health of the hosts, containers and applications monitored by skym. Use when asked how the servers are, what is wrong or broken, why something failed, or whether a deployment went well.
---

# skym

`SKYM_URL` and `SKYM_TOKEN` come from the environment; if unset, read them from `~/.config/skym/env`.

```sh
curl -fsS -H "Authorization: Bearer $SKYM_TOKEN" "$SKYM_URL/api/overview"
```

`GET /api` lists every endpoint and its parameters.

## How to read it

1. Start at `/api/overview`. `problems` lists every open, unmuted incident, worst and oldest first; each names its application in `app` (none for a host's own: heartbeat, disks, logs). Name problems by their application, then the host. A workload's problem carries the workload (`workload`: image, ports, memory against its limit, restarts, a failing check's output), and each application its `workloads`, latest `deploys` and `exceptions_1h`: answer from these before following `links`. `hosts` (also `/api/hosts`) gives each machine's load, memory and disks. A URL of an application down (`ENDPOINT_DOWN`, `CERT_EXPIRING`) while its services are fine points at what lies between: a reverse proxy, DNS, a certificate.
2. Asked which applications there are, or about one by name: start at `/api/apps`. Each has a `name`, an `env` and a `note` saying what it is; quote the note. Point out applications without an `env` or `note`, so someone describes them. Asked about a customer or a group, select by `tags` (on applications and hosts); about one machine, by the host in the application's `key` (`<host>/...`). `APP_MISSING` means a configured application has nothing running on its host.
3. `HEARTBEAT_LOST` comes first: nothing else about that host is current. When the details say "all hosts silent", suspect the server or its network, not every host.
4. Follow `links` to drill down (host → workload → timeline, exceptions, incidents); build a URL only from what `GET /api` lists.
5. `open_for` is how long skym has seen a problem, not how long it has existed: when it is close to the host's `observed_since`, the problem predates skym. An incident's `since`, when present, is when it really began. Lead with problems that are new.
6. Severity, thresholds and status are already judged: report them, do not re-derive them from raw numbers. `info` incidents are hygiene (e.g. unbounded logs): mention them last, briefly.
7. Explain with the timeline: a `deployed` or `config_changed` event just before an incident is the first suspect.
8. Exceptions: `/api/exceptions?host=…` groups failures by component and code; `business` ones are expected outcomes, not faults.

On a monitored host itself, `skym status --json` and `skym exceptions` give the same view without the server, in more detail.

Answer with what is wrong, since when, the likely cause, and what to look at next. Say so when everything is fine.
