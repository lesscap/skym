---
name: skym
description: Check the health of the hosts, containers and applications monitored by skym. Use when asked how the servers are, what is wrong or broken, why something failed, or whether a deployment went well.
---

# skym

A read-only JSON API over every watched host, its containers and systemd units (workloads), the applications they form, and the URLs probed for them. `SKYM_URL` and `SKYM_TOKEN` come from the environment, else from `~/.config/skym/env`.

```sh
curl -fsS -H "Authorization: Bearer $SKYM_TOKEN" "$SKYM_URL/api/overview"
```

## Endpoints

- `/api/overview`: what is wrong now (`problems`), and every host in brief.
- `/api/apps`, `/api/apps/{host}/{project}`: applications, their workloads, URLs, deploys and exception counts.
- `/api/hosts`, `/api/hosts/{host}`: hosts, their resources, disks and workloads.
- `/api/hosts/{host}/workloads/{project}/{service}`: one workload in full, with its recent events and exceptions.
- `/api/timeline?host=…`: incident changes and events (deploys, restarts, OOM kills, reboots) by time.
- `/api/incidents`: incident history, resolved and muted ones included on request.
- `/api/exceptions?host=…`: failures applications reported, grouped, with a sample.

Responses carry `links` to drill down; `GET /api` lists parameters. Fields, parameters and incident codes: [reference.md](reference.md).

## Reading it

- Times are UTC.
- Severity and status are skym's verdicts: report them, do not re-derive them from raw numbers. `info` is hygiene, not a fault.
- A host with `HEARTBEAT_LOST` has no current data. "All hosts silent" in its detail points at the server or its network.
- `open_for` is how long skym has seen a problem; `since`, when present, is when it began, possibly before skym (`observed_since`) watched.
- Muted incidents are known and accepted: they leave the overview (counted in `muted_count`) and count towards no status, so a muted workload may show `ok`.
- `HEALTHCHECK_BROKEN`: the image's check cannot run, so the workload's health is unknown, not bad.
- `business` exceptions are expected outcomes, not faults.
- `other_cpu_cores` and `other_memory_bytes` are what a host uses outside its workloads; the memory is a lower bound.

## What skym cannot tell

- History of CPU, memory or network: only the latest report is kept (disks keep a short series for the projection).
- Logs: only grouped exception signals.
- Processes, listening sockets, anything inside a container, or hosts it does not watch.
- Who did something (a deploy, a restart): only what changed and when.
- It cannot act: no restarts, no mutes (those live in the server configuration).

Say so rather than guess, and point at the host itself. On a watched host, `skym status --json` and `skym exceptions` show the same view in more detail.

Running on a schedule to watch on your own? Read [watching.md](watching.md).
