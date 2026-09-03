# fast_exit_delivery — a process that makes one call and exits still pings

Proves the property the first-request heartbeat trigger exists for: a **short-lived
process** — a CLI, a Lambda handler, a CI step — delivers its telemetry ping.

```bash
./test.sh          # 12 runs by default; RUNS=n to change
```

## Why 12 runs and not 1

Because the defect is a **race**, and a single run passes against it roughly one time
in twelve.

Before the cold-path send was awaited inline, it was spawned onto the tokio runtime.
A process that returns from `main` drops the runtime, and the in-flight POST is
cancelled. Measured on this fixture: **2 deliveries in 12** under the spawned
implementation, **12 in 12** once awaited.

The `health_fetches` figure in the output is the part worth reading twice. Under the
spawned implementation it was **9** — so nine runs contacted the customer's own
platform with a `/health` GET and then recorded nothing for it. That is worse than no
telemetry: an unsolicited request to someone else's server, with no data to show for
it.

## What the fixture must not have

No `sleep`, no join, no flush after the call. Any of them would let a spawned send
finish, and the fixture would read as a **disproof of a bug it never gave itself a
chance to see**. Each run also gets a private `HOME`/`XDG_CACHE_HOME`, because the
7-day stamp would otherwise suppress every run after the first and the whole thing
would pass vacuously.

The listener checks the **path**, not just the method: the driver's own API call must
not be counted as a `/health` fetch.

## Relationship to the unit suite

`heartbeat::tests::the_cold_path_send_is_awaited_not_spawned` covers the same defect
in-process, and needed a specific shape to do it: every other heartbeat test uses
`await_ping`, which **polls** — and polling cannot tell an awaited send from a spawned
one, because it waits for the spawned one too. That is why the whole unit suite stayed
green under this mutant until a non-polling assertion was added.

This driver is the end-to-end confirmation against a real compiled binary that really
exits.
