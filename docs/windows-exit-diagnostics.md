# Windows artifact quit: readiness, delivery, shutdown and exit

This is a disposable-runner interface, not a shared-host reproduction recipe.
The actual app tests remain ignored and require `BILILIVE_ARTIFACT_OK=1`.
No new process enumeration or termination mechanism is added.

## Baseline and falsifiable hypotheses

Run `35855030445` installed MSI successfully, then `taskkill /PID` reported
success, but the app-exit assertion failed. App stdout/stderr were discarded;
there is no evidence in that old run distinguishing delivery from shutdown.

1. **Readiness race:** the test waited only for a TCP listener. The app creates
   its Tauri window *after* backend API readiness and waits another 1.2 seconds
   before showing it. A quit request can therefore precede its GUI recipient.
   Prediction: requiring the actual GUI-ready marker before the driver, then
   receiving CloseRequested, removes this race.
2. **Driver did not deliver:** even with GUI ready, `taskkill` success might not
   mean the main window received CloseRequested. Prediction: ready marker but
   no close marker. Fail here, not later as an undifferentiated app timeout.
3. **Shutdown hangs:** the callback runs but guardian shutdown does not return.
   Prediction: close-requested + stop-backend-start, without returned marker.
4. **Runtime exit stalls:** shutdown returned and exit was invoked, but the
   owned app process does not exit. Prediction: exit-invoked without successful
   child wait. A run-exit marker alone is not proof of process termination.

These are hypotheses until the new runner evidence is collected, not a claim
that the old failure was conclusively diagnosed from a successful taskkill.

## Opt-in observation interface

The artifact parent creates a unique directory below its evidence directory
(or its temporary directory), a fresh `lifecycle.log`, and `app-output.log`.
It passes the trace path as `BILILIVE_ARTIFACT_TRACE` and gives the app a fresh
recording work directory inside that run directory. App stdout/stderr now go
to the per-run log instead of being discarded. Existing CI evidence upload
recursively captures these files; no workflow changes are needed.

`artifact_probe::record` writes only when `BILILIVE_ARTIFACT_OK=1` and the trace
path is set. It appends to an existing file, never creates one. Each complete
line is `<app PID> <event>`. Test readers require this run's owned child PID,
the exact event, and a newline; another process or incomplete line cannot
acknowledge a request. Ordinary app launches do not open a trace file.

Markers: `backend-ready`, `main-window-created`, `main-window-ready`,
`close-requested`, `stop-backend-start`, `guardian-wait-exited` or
`guardian-wait-unsettled`, `stop-backend-returned`, `exit-invoked`,
`run-exit-requested`, `run-exit`.

The Windows normal-quit driver now waits for `main-window-ready`, runs the
existing `taskkill /PID` against its own spawned app, and requires the app's
`close-requested` acknowledgment. Ready and acknowledgment waits are bounded.
Successful driver return without acknowledgment is explicitly a failure.
An acknowledged close is still not a pass: the existing owned-process exit
and backend-service checks must independently succeed. Failure output names
the last observed phase; Drop prints the trace before fixture cleanup.

Guardian shutdown decisions, wait-error handling, platform containment and
ordinary app close behavior are unchanged. No file-integrity guarantee is
inferred from these markers, guardian return, port state, or CLI log messages.

## Signal-free local regression

`cargo test --manifest-path src-tauri/Cargo.toml --test artifact_lifecycle quit_handshake_tests`

The production test-driver helper is exercised with a callback spy and tiny
trace files: TCP/backend readiness alone must issue zero quit requests; a
successful driver without a close marker must fail; a delivered close must
be distinguished from a still-pending shutdown. Before adding the readiness
gate, the first regression failed with request count 1 vs expected 0.

`cargo test --manifest-path src-tauri/Cargo.toml --lib artifact_probe`

Pins PID/event/complete-line matching and failure-stage classification. These
local tests do not start an app, send a signal, or terminate any process. The
real installed-app cases must run only in the existing disposable CI jobs.
