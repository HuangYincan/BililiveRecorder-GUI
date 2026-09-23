#!/bin/bash
# End-to-end check that the app never leaves its backend running, on macOS.
#
# Installs nothing: point it at an already-installed (or DMG-copied) bundle.
# Each scenario launches that bundle, delivers one kind of termination, and then
# reports whether the bundled CLI is gone. An independent recorder — a second
# copy of the same CLI started by this script and standing in for a process the
# user runs themselves — must survive every scenario.
#
# SAFETY: this script only ever signals a PID it has verified is the GUI it
# launched, or a descendant of that GUI. It never signals by process name.
#
# Usage: scripts/verify-sidecar-lifecycle.sh "<path to BililiveRecorder GUI.app>" [scenario]
#        scenario = quit | close | sigterm | sigkill | all   (default: all)

set -u

APP="${1:-}"
SCENARIO="${2:-all}"
if [ -z "$APP" ] || [ ! -d "$APP" ]; then
  echo "usage: $0 \"<path to BililiveRecorder GUI.app>\" [quit|close|sigterm|sigkill|all]" >&2
  exit 2
fi
APP=$(cd "$APP" && pwd)

WORK=$(mktemp -d "${TMPDIR:-/tmp}/bililive-lifecycle.XXXXXX")
GUI_MATCH="$APP/Contents/MacOS/bililive-recorder-gui"
SIDECAR_BIN=$(find "$APP/Contents/Resources/sidecar" -name 'BililiveRecorder.Cli' -type f | head -1)
if [ -z "$SIDECAR_BIN" ]; then echo "no bundled CLI found under $APP/Contents/Resources/sidecar" >&2; exit 2; fi
LOG="$WORK/verify.log"; : > "$LOG"
log() { echo "$@" | tee -a "$LOG"; }

alive() { [ -n "$1" ] && ps -p "$1" >/dev/null 2>&1; }
cmd_of() { ps -p "$1" -o command= 2>/dev/null; }
# $1 pid, $2 must-contain, $3 must-not-contain -> first matching child pid
child_of() {
  ps -ax -o pid=,ppid=,command= | awk -v p="$1" '$2==p' \
    | grep -F -e "$2" | grep -vF -e "${3:-@@none@@}" | awk '{print $1}' | head -1
}

# A recorder this script started: it must survive every scenario below.
mkdir -p "$WORK/decoy" "$WORK/decoydata"
cp -R "$(dirname "$SIDECAR_BIN")/." "$WORK/decoy/"
nohup "$WORK/decoy/BililiveRecorder.Cli" run --bind "http://127.0.0.1:51999" "$WORK/decoydata" \
  >"$WORK/decoy.log" 2>&1 &
DECOY_PID=$!
sleep 4
alive "$DECOY_PID" || { log "FATAL: the independent recorder did not start:"; tail -5 "$WORK/decoy.log" | tee -a "$LOG"; exit 1; }
log "independent recorder (must never be touched): pid=$DECOY_PID"

GUI_PID=""; SIDECAR_PID=""; GUARDIAN_PID=""

start_app() {
  local before after pid
  xattr -dr com.apple.quarantine "$APP" 2>/dev/null
  before=$(mktemp); after=$(mktemp)
  pgrep -f "$GUI_MATCH" | sort > "$before" || true
  open -n "$APP"
  GUI_PID=""
  for _ in $(seq 1 90); do
    pgrep -f "$GUI_MATCH" | sort > "$after" || true
    for pid in $(comm -13 "$before" "$after"); do
      if [ -n "$(child_of "$pid" 'BililiveRecorder.Cli')" ]; then GUI_PID=$pid; break; fi
    done
    [ -n "$GUI_PID" ] && break
    sleep 0.5
  done
  rm -f "$before" "$after"
  sleep 2
  case "$(cmd_of "$GUI_PID")" in
    *"$GUI_MATCH"*) ;;
    *) log "  ABORT: could not verify the GUI process (pid='$GUI_PID')"; GUI_PID=""; return ;;
  esac
  SIDECAR_PID=$(child_of "$GUI_PID" 'BililiveRecorder.Cli' '--bililive-sidecar-guardian')
  GUARDIAN_PID=$(child_of "$GUI_PID" '--bililive-sidecar-guardian')
  log "  gui=$GUI_PID sidecar=${SIDECAR_PID:-none} guardian=${GUARDIAN_PID:-none}"
}

signal_gui() { # $1 = signal number, refuses anything unverified
  case "$(cmd_of "$GUI_PID")" in
    *"$GUI_MATCH"*) kill -"$1" "$GUI_PID" ;;
    *) log "  REFUSING to signal unverified pid '$GUI_PID'" ;;
  esac
}

check() {
  local label="$1" i gui_exited=no
  for i in $(seq 1 40); do alive "$GUI_PID" || { gui_exited=yes; break; }; sleep 0.5; done
  log "  gui[$label]: $([ "$gui_exited" = yes ] && echo exited || echo 'STILL RUNNING after 20s')"
  for i in $(seq 1 20); do alive "$SIDECAR_PID" || break; sleep 0.5; done
  sleep 1
  if alive "$SIDECAR_PID"; then
    if [ "$gui_exited" = no ] && { [ "$label" = quit ] || [ "$label" = close ]; }; then
      log "  RESULT[$label]: INCONCLUSIVE - the $label action never reached the app"
    else
      log "  RESULT[$label]: FAIL - sidecar $SIDECAR_PID survived (ppid=$(ps -p "$SIDECAR_PID" -o ppid= | tr -d ' '))"
    fi
  else
    log "  RESULT[$label]: PASS - sidecar $SIDECAR_PID reaped"
  fi
  if [ -n "$GUARDIAN_PID" ]; then
    for i in $(seq 1 20); do alive "$GUARDIAN_PID" || break; sleep 0.5; done
    alive "$GUARDIAN_PID" && log "  guardian[$label]: STILL RUNNING (leak)" || log "  guardian[$label]: exited"
  fi
  alive "$DECOY_PID" && log "  independent[$label]: OK - untouched" || log "  independent[$label]: *** KILLED ***"
}

cleanup() { for p in $GUI_PID $SIDECAR_PID $GUARDIAN_PID; do alive "$p" && kill -9 "$p" 2>/dev/null; done; sleep 1; }

run_scenario() {
  GUI_PID=""; SIDECAR_PID=""; GUARDIAN_PID=""
  log "=== scenario: $1 ==="
  start_app
  [ -z "$GUI_PID" ] && { log "  FAIL: the app did not start"; cleanup; return; }
  [ -z "$SIDECAR_PID" ] && { log "  FAIL: no sidecar started"; cleanup; return; }
  case "$1" in
    quit)    log "  action: Apple Event quit (osascript quit)"
             osascript -e "quit app \"${APP##*/}\"" >/dev/null 2>&1 ;;
    close)   log "  action: close main window (red close button)"
             osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $GUI_PID) to true" >/dev/null 2>&1
             sleep 1
             osascript -e "tell application \"System Events\" to click button 1 of window 1 of (first process whose unix id is $GUI_PID)" >/dev/null 2>&1 ;;
    sigterm) log "  action: SIGTERM gui $GUI_PID"; signal_gui 15 ;;
    sigkill) log "  action: SIGKILL gui $GUI_PID"; signal_gui 9 ;;
  esac
  check "$1"; cleanup
}

if [ "$SCENARIO" = "all" ]; then for m in quit close sigterm sigkill; do run_scenario "$m"; done
else run_scenario "$SCENARIO"; fi

log "=== independent recorder final ==="
alive "$DECOY_PID" && log "  pid $DECOY_PID still alive (expected)" || log "  GONE (not expected)"
kill -9 "$DECOY_PID" 2>/dev/null
log "log: $LOG"
log "=== done ==="
