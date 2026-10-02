# Exception protocol

Applications report failures by writing one JSON line to stdout or stderr. `skym` reads container logs on the same host, groups the lines, and includes the groups in its [report](report-protocol.md).

The application does not depend on `skym`: no SDK, no socket, no endpoint. If `skym` is absent or down, nothing changes for the application.

## A line

```json
{"skym":"exception","class":"application","component":"structurize","code":"LLM_TIMEOUT","message":"upstream timed out after 30s","biz_key":"notice:88123","attempt":2,"final":false,"exception.type":"TimeoutError","exception.stacktrace":"TimeoutError: ...\n    at ..."}
```

With pino:

```ts
logger.error({
  skym: 'exception',
  class: 'application',
  component: 'structurize',
  code: 'LLM_TIMEOUT',
  biz_key: `notice:${notice.id}`,
  attempt,
  final: attempt >= maxAttempts,
  'exception.type': err.name,
  'exception.stacktrace': err.stack,
}, err.message)
```

The whole record must be on one line. Newlines inside values are escaped as JSON requires.

## Fields

| Field | Required | Rule |
| --- | --- | --- |
| `skym` | yes | `exception`. Marks the line as an exception line. |
| `class` | yes | `application` or `business`, see below. |
| `component` | yes | Subsystem name. `[a-z0-9_.-]`, at most 64 characters, stable over time. |
| `code` | yes | Error category. `UPPER_SNAKE`, at most 64 characters. Must not contain IDs, numbers or anything that varies per occurrence. Codes starting with `_` are reserved for `skym`. |
| `message` or `msg` | recommended | Human-readable summary. Truncated to 1000 characters. `msg` is accepted so that pino's default output works. |
| `biz_key` | recommended | The business entity that failed, e.g. `notice:88123`. |
| `attempt` | optional | Attempt number, starting at 1. |
| `final` | optional | `true` when the failure will not be retried. Defaults to `true`: a failure without retry is final. |
| `exception.type` | optional | Exception class name (OpenTelemetry semantic convention). |
| `exception.message` | optional | Exception message (OpenTelemetry). |
| `exception.stacktrace` | optional | Stack trace (OpenTelemetry). Truncated to 8 KB. |

Not needed in the line: a timestamp (Docker records one per line), a log level (`class` carries the meaning), host or service names (`skym` knows the container).

Unknown fields are ignored. Do not put secrets, tokens, payloads or personal data in any field.

### `class`

| | `application` | `business` |
| --- | --- | --- |
| Meaning | Unexpected. Someone should look. | Expected domain outcome. |
| Examples | `LLM_TIMEOUT`, `DB_ERROR`, unhandled exception, task given up | `SOURCE_MISSING`, `REJECTED`, `VALIDATION_FAILED` |
| Effect | Can open the `APP_EXCEPTIONS` incident | Counted and queryable, never an incident |

Rule of thumb: if an on-call engineer would say "that shouldn't happen", it is `application`; if they would say "that input was just bad", it is `business`.

Severity is not part of the protocol. It is decided by `skym`'s judgement rules so that every application is judged the same way.

## Grouping

```text
group_key = (workload, class, component, code)
```

For each group and report interval, `skym` sends:

| Field | Meaning |
| --- | --- |
| `count` | Lines in the interval |
| `final_count` | Lines with `final = true` |
| `first_seen`, `last_seen` | Docker timestamps of the first and last line |
| `biz_keys` | Up to 5 most recent distinct `biz_key` values |
| `sample` | `application` only: the most recent line's message and stack trace, truncated and redacted |

`biz_key` is not part of the group key; it would create one group per entity.

`business` groups carry no sample. They are usually numerous and their details belong to the application's own records.

## Limits and protocol errors

| Situation | Handling |
| --- | --- |
| More than 100 groups for one workload in one interval | Extra lines go to `code = "_OVERFLOW"` |
| `skym` with a value this `skym` does not know (a line kind added later) | Ignored |
| Marked line that is not valid JSON, misses a required field, or has an invalid `component` / `code` | Counted under `code = "_PROTOCOL_ERROR"` so the application owner sees it; it never opens an incident |

## Unmarked stderr lines

Lines without the marker are not protocol lines. On stdout they are ignored. On stderr they are a fallback signal for crashes and errors the application did not report itself. Many runtimes write all their logs to stderr (Python's `logging` does by default), so only lines that look like a failure count:

```text
looks_like_error(line) ⇔ line matches (?i)(error|exception|panic|panicked|fatal|traceback|unhandled)\b
                        ∧ line has no lower level: no logfmt/JSON level warn|info|debug|trace,
                          and no INFO|DEBUG|TRACE|WARN|WARNING word within its first 40 characters

an error-like line starts a record; following indented lines join it (stack traces);
a Python traceback is one record: an error line logged right before it (logger.exception)
  stays its message; otherwise its exception line (the first unindented line after the frames)
  is the message; the exception line is the fingerprint; chained tracebacks ("During handling
  of the above exception…") continue the record and the last exception names it;
any other stderr line ends it
fingerprint = hash(first line with numbers, hex strings, UUIDs and quoted text replaced)
group_key   = (workload, application, "_stderr", "_UNSTRUCTURED:" + fingerprint[0..8])
```

When an application adopts the protocol, its failures move from `_stderr` to their own components and codes.

## Redaction

Before anything leaves the host, `skym` truncates messages and stack traces and replaces values that look like secrets (bearer tokens, `password=`, connection strings with credentials, private keys). Redaction is a safety net; the application is still responsible for not logging secrets.

## Compatibility

The protocol has no version number. It grows by adding optional fields; existing fields never change meaning. See [evolution](architecture.md#evolution).
