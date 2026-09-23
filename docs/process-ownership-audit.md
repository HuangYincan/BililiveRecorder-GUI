# Process wait / termination call-site audit

Scope: PR #3 production lifecycle (`src-tauri/src`), with test-driver sites
listed separately below. No process enumeration or user-instance termination
is part of this audit. Function names are the stable anchors; line numbers are
historical; function names are authoritative after subsequent edits. This is an ownership inventory, not cross-platform runtime
proof.

## Preconditions (not PID re-querying)

- The app exclusively owns/reaps its spawned guardian. The guardian exclusively
  owns/reaps its spawned backend. Neither installs an auto-reaping SIGCHLD
  disposition nor intentionally delegates reaping. A Unix `Child` value alone
  does **not** prevent another thread/library from calling raw `waitpid`.
- A fresh spawn or `Ok(None)` is usable only under that sole-reaper assumption.
  A successful reap or any observed wait error ends permission to signal that
  child. Repeating PID queries is not a substitute. An arbitrary concurrent
  external reaper between a successful poll and a signal is **not** solved by
  these changes; no pidfd or other new architecture is introduced here.
- Windows uses owned process handles; Linux parent-death and Windows Job Object
  containment are kernel mechanisms, not signals to remembered backend PIDs.
  macOS has no equivalent containment in this implementation.

## Production: complete operation inventory

All `lib.rs` locations below are under `src-tauri/src/`.

| Site | Operation and ownership premise / failure action |
| --- | --- |
| `lib.rs:367,375,379`, `SupervisedChild for Child` | Delegates `try_wait`, `kill`, `wait` to std. `interrupt` (:370) delegates to `send_interrupt`; these adapters establish no ownership themselves. The private interface is also used by the operation-counting spy. |
| `lib.rs:315`, `send_interrupt` | Unix `kill(SIGINT)` of the supplied child ID; Windows delegates to `platform::interrupt_backend`, scoped to the new backend control group in the guardian's private console. Only caller is the adapter used by `shutdown_child`, with the preconditions below. |
| `lib.rs:389-402`, `wait_for_exit` → `wait_for_child_exit` | Polls owned child; `Some` → Exited/reaped, `Err` → Unwaitable immediately, `None` at deadline → TimedOut. No signal. Used by guardian shutdown twice and backend shutdown once. |
| `lib.rs:436`, first guardian bounded wait | Own app-spawned guardian. Exited/Unwaitable return without escalation. macOS also returns on TimedOut; only containment-enabled platforms proceed. |
| `lib.rs:459`, guardian SIGTERM | Unix containment-enabled path (Linux); first wait must return TimedOut. Sole-reaper assumption must still hold. No backend PID is queried or signalled. |
| `lib.rs:462`, second guardian bounded wait | Exited/Unwaitable return; only another TimedOut permits hard termination. |
| `lib.rs:471-475`, guardian `kill` then `wait` | Second confirmed timeout plus sole ownership. Final blocking wait error returns None; no subsequent signal. A failed kill can leave the blocking wait pending (existing behavior, not newly solved). |
| `lib.rs:484-500`, backend `interrupt`, bounded wait, `kill`, `wait` | Entered only on keepalive EOF before the monitor has observed a terminal wait result (fresh spawn or earlier `Ok(None)`). After a successful interrupt, Exited/Unwaitable return; only TimedOut escalates. If interrupt is unavailable/fails, immediate kill uses the same entry ownership premise, **not** a wait error. Final wait does not trigger further signals; unsuccessful termination can leave that wait pending. |
| `lib.rs:623`, `monitor_child` poll | The actual loop called by `supervise_with`, borrowing the just-spawned backend. Some → exit event/return; None → continue; **Err → error event/return 1, zero interrupt/kill/wait calls**, including if EOF becomes true during the error. It must not re-enter shutdown. |
| `lib.rs:863`, `stop_backend` diagnostic poll | After unsuccessful guardian shutdown, macOS checks whether to warn. May reap an exited guardian; no result licenses a signal. No retry/escalation follows. |
| `platform.rs:46-53`, Linux parent-death setup | In the new child's pre-exec callback, arms SIGKILL on death of the forking guardian thread; checks parent race and `_exit(1)` if already orphaned. No external PID receives a userspace signal. |
| `platform.rs:113,119`, Job Object failure closes | Closes newly created job on setup/assignment failure, before any backend spawn. Success deliberately does not CloseHandle; raw handle has no Drop. Kernel closes it on guardian exit. Removed Copy-value `mem::forget`, which did not implement this lifetime. |

### Windows graceful-control addition

- `platform::private_console` runs before any backend spawn. `AllocConsole`
  creates only the guardian's console; there is no AttachConsole or enumeration.
  The new console is hidden using its own GetConsoleWindow handle. All three
  inherited standard handles are saved/restored so the keepalive and event
  pipes remain unchanged. Any setup error refuses the spawn.
- `platform::configure_backend` sets CREATE_NEW_PROCESS_GROUP, not
  CREATE_NO_WINDOW: the backend inherits that private console but starts its
  own control group. The guardian is not in the backend's group.
- `platform::interrupt_backend` uses GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT,
  child.id()). The caller still owns an unreaped child created with the above
  configuration. Group zero is explicitly rejected; no broadcast or PID query
  is used. The control event addresses the backend's group (including any of
  its same-console descendants that inherit the group), not arbitrary processes
  attached to a user's console. Existing Exited/Unwaitable stop conditions and
  timeout-only escalation after a successful interrupt remain unchanged.
- The Job Object remains kill-on-close containment, not the normal shutdown
  mechanism. The normal artifact case now also requires a zero-code guardian
  exit, so accidentally terminating the guardian with a control event cannot
  masquerade as a normal shutdown.
- Opt-in artifact control uses only the parent's own child stdin to request
  the Tauri main window's normal CloseRequested path; trace files do not issue
  process signals. The production GUI/guardian wait decisions are unchanged.

Additional lifecycle requests: `stop_backend` closes its own guardian stdin
(:847), asking the guardian to stop via EOF, not a PID signal. The keepalive
reader waits on its own pipe. Signal handlers record received signals and ask
Tauri to exit; they do not send process signals. Thread sleeps, HTTP readiness
polls and child `Command::status/output` helper waits do not authorize lifecycle
termination. There are no other production process wait/signal sites in
`src-tauri/src`.

## Tests and drivers (not production guarantees)

- `lib.rs:1179`: unit-test status poll after shutting down its own stand-in.
  New `monitor_tests` use **no OS child**, inject monitor wait failure and count
  interrupt/kill/wait operations. Three error scenarios expect zero termination
  calls; EOF positive control expects interrupt + kill + wait.
- `tests/sidecar_lifecycle.rs:149`: `kill(pid, 0)` is an existence probe only,
  never permission to terminate or proof against PID reuse.
- Same file :267-268, :305-306, :406-407: kill/wait cleanup of test-created
  unrelated stand-ins, never a real user's recorder. :424/:432: kill then reap
  a directly spawned child. :458/:463: kill then raw `waitpid` to manufacture
  ECHILD; **no subsequent signal** is sent to that released child ID.
- Same file :522-523: kill/wait after confirmed timeout of its own stand-in;
  :544 diagnostic `try_wait`; :740-741 cleanup of its own deliberately hung
  guardian under exclusive-reaper assumptions. Calls to production
  `wait_for_exit`/`shutdown_guardian` retain the production rules above.
- `tests/artifact_lifecycle.rs:189`: Apple Event quit **by app name**; valid
  only on the disposable runner with the dedicated app installed. :199:
  `taskkill /PID` for its directly spawned unreaped app, not proof that a GUI
  close callback was delivered. :218 SIGTERM, :223 hard kill (:368 caller),
  :248-249 Drop cleanup: dedicated child only, no external reaper allowed.
- Same file :305 bounded wait, :426-427 stand-in cleanup. This **test helper**
  still conflates timeout/error, and Drop does not separately reject an
  unexpected wait error. It is not a fail-closed production implementation or
  permission to run real-artifact termination on a shared host. The real-app
  cases remain ignored by default and were not executed locally in this fix.
- `.github/workflows/build.yml` only invokes those artifact drivers on
  disposable CI runners. No additional raw signal or process-wait sites occur
  in `scripts/` or the workflows.

## Regression evidence

`cargo test --manifest-path src-tauri/Cargo.toml --lib monitor_tests` exercises
the real extracted post-spawn loop through the same process-operation interface
as production. With the old error-arm kill/wait retained, the failure test
actually failed: calls `["try_wait", "kill", "wait"]`, termination count 1 vs
expected 0. After deleting that fallback it passes for immediate error, error
after a running poll, and simultaneous EOF. The positive control proves the
spy would observe termination, rather than testing only an outcome enum.

The private seam does not change command creation, containment, IPC, timeouts,
normal shutdown policy, workflow wiring, or platform support boundaries.
