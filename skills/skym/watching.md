# Watching

For an agent that runs on a schedule, reads skym on its own and decides what is worth telling. Answering questions is in [SKILL.md](SKILL.md); read it first. The wording, language and layout of messages belong to your deployment.

## Each run

1. **Were you away?** If your last run (in your memory, below) is older than two intervals, say so first, and what happened meanwhile: `GET /api/incidents?status=resolved&since=<last run>` and, per host, `GET /api/timeline?host=<host>&since=<last run>`.
2. **Read `GET /api/overview`.** Unreachable, or `401`: say so, once per outage, and stop. Its `problems` are skym's verdicts: open, unmuted, worst and oldest first.
3. **Compare with what you told.** An incident is identified by `(subject, code, opened_at)`; it keeps its `opened_at` when it reopens within 30 minutes.
   - New: `warn` or `critical`, not told: tell.
   - Worse: its severity rose since you told it: tell.
   - Resolved: told, and now in `GET /api/incidents?status=resolved&since=<last run>` with a `resolved_at` at least 10 minutes old: tell. Until then it may reopen as the same incident; one that flaps says nothing. One merely gone from `problems` may have been muted: drop it without a word.
   - Muted, or `info`: never tell, unless asked.
4. **Look beyond the verdicts.** This is what you add; say it is your reading, not skym's.
   - A `deployed` or `config_changed` event shortly before exceptions rise or an incident opens (`/api/timeline`).
   - `exceptions_1h` of an application well above what you saw in earlier runs.
   - A disk losing free space run after run, before skym's projection calls it.
   - Restarts in the last hour climbing on a workload without an incident.
   - A host whose `other_cpu_cores` or `other_memory_bytes` is most of its use: something outside its workloads.
   - Several URLs of one application down while its services run: a reverse proxy, DNS or a certificate between them.
   - Many hosts losing their heartbeat at once: the server or its network, not each host.
5. **Decide.** Speak only when something is new: one message, worst first, problems with one cause told as one. Otherwise say nothing. Once a day, a short digest: what opened and resolved, what is still open and for how long (long-standing ones in one line), anything you noticed.
6. **Remember**, then signal that you ran (a dead man's switch such as healthchecks.io, if your deployment has one), so that your own silence is noticed.

## Memory

A small file you rewrite each run, for example:

```json
{
  "last_run": "2026-10-05T09:30:00Z",
  "told": [
    { "subject": "workload:x/shop/api", "code": "WORKLOAD_DOWN",
      "opened_at": "2026-10-05T09:12:00Z", "severity": "critical" }
  ],
  "seen": { "x/shop": { "exceptions_1h": 3 }, "i:/data": { "free_bytes": 52000000000 } },
  "away_told": false
}
```

Keep only what the next run compares against.

## Don't

- Re-derive thresholds from raw numbers: severity and status are judged already.
- Repeat a problem you told and that has not changed.
- Leave out long-standing problems from the digest: one line is enough.
- Suggest a remedy without evidence for it; say what to look at instead.

## Cost

`/api/overview` is enough for most runs. Drill down (`links`, `/api/apps`, `/api/timeline`) only into what you are about to tell.
