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

Run `35858098177` (head `2c78cc1`) now separates these hypotheses: the
Windows normal-quit trace contains backend-ready, main-window-created and
main-window-ready, taskkill reports success, but CloseRequested is never
observed during the bounded acknowledgment wait. Neither shutdown-start nor
exit markers appear, and the app-output log contains no trace-write error.
Thus readiness alone did not fix this: the request did not reach our app's
close handler; there is no observed guardian-shutdown hang in this path.
The separately executed forced-kill case passed 1/1 and reported no clean CLI
shutdown. That is not recording-file integrity evidence.

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
`close-command-received`, `close-command-posted`, `close-requested`, `stop-backend-start`, `guardian-wait-exited` or
`guardian-wait-unsettled`, `stop-backend-returned`, `exit-invoked`,
`run-exit-requested`, `run-exit`.

The corrected Windows normal-quit driver waits for `main-window-ready`, then
writes one fixed command to its own `Child::stdin` pipe. Only an app opted in
with both artifact variables installs the reader. It addresses the app's
logical `main` window through `WebviewWindow::close()`, which traverses the
real CloseRequested callback; it does NOT call destroy(), stop_backend() or
app.exit() as a shortcut. There is no network control endpoint, raw HWND/PID
lookup, or process/window enumeration. EOF/unknown input does not request exit.

The test then requires the app's `close-requested` acknowledgment. Ready and
acknowledgment waits are bounded. A successful pipe write or posted close
without the callback is explicitly a failure. The intermediate command markers
distinguish pipe receipt from runtime delivery. This interface tests the real
Tauri normal-close path, not taskkill behavior or OS-level mouse injection.
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

## Backend graceful shutdown after the GUI delivery fix

Run `35860526134` reached the real CloseRequested callback, guardian wait,
RunEvent::Exit and app process exit. Its remaining Windows failure was the
CLI clean-shutdown log assertion: the console-less backend had no graceful
interrupt route and fell straight to Child::kill.

The next implementation allocates a hidden **guardian-owned** console before
spawn, preserving all inherited standard handles. It starts the backend with
CREATE_NEW_PROCESS_GROUP in that console and targets CTRL_BREAK_EVENT only at
that group's owned root PID; group zero and existing-console attachment are
not used. Upstream v2.20.0 `BililiveRecorder.Cli/Program.cs:326-335` registers
Console.CancelKeyPress before host startup; the handler cancels the token, then
its shutdown path stops the host and disposes the recorder. The existing
bounded wait/escalation and Job Object abrupt-death fallback are retained.

The test's clean-shutdown assertion is NOT removed. Normal Windows close also
requires `guardian-exit-success`, separating graceful child exit from an
accidentally terminated guardian. This does not add recording-file evidence;
that remains a separate gap even if all these control-flow checks pass.

The Windows normal-close case additionally requires the guardian's actual
`backend-graceful-exit` marker (bounded wait returned child exit code 0) and
rejects ANY `backend-hard-kill` attempt in that run. GUI/guardian success and
an early clean-shutdown log can no longer hide timeout escalation. This is
stronger than the old log/port predicate and does not certify recording files.

The first private-console run (`35865807259`) refused startup at AllocConsole
with Windows error 5 while the guardian still used CREATE_NO_WINDOW. Hidden
window creation and detached console creation are distinct. The guardian is
now spawned with DETACHED_PROCESS and its existing explicit standard pipes;
AllocConsole must then succeed before any backend starts. There is still no
fallback that adopts an existing console. The backend, separately, uses only
CREATE_NEW_PROCESS_GROUP so it inherits the newly allocated private console.
