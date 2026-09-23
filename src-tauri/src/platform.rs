//! The places where the guardian's guarantees need the operating system.
//!
//! Everything here is kept in one module that depends on nothing but `std` and
//! raw bindings, so that it can be type-checked for every target even though
//! the crate as a whole cannot be: `cargo check` needs no linker, but tauri's
//! C dependencies need a real sysroot for the target (`windows.h`, GTK), so
//! cross-checking the whole crate fails before it ever reaches Rust code. This
//! module has no such dependency and *is* checked for Linux and Windows.
//!
//! Being type-checked is not being tested. See the platform support boundary in
//! `lib.rs`.
//!
//! # The rule this module exists to keep
//!
//! Unix signals address an owned, unreaped child. Windows shutdown addresses
//! the NEW_PROCESS_GROUP rooted at that owned child inside the guardian's new
//! private console. It never attaches to another console or broadcasts to group
//! zero. The existing Job Object remains the abrupt-death containment fallback.

use std::process::Command;

/// Whether the kernel guarantees this process's descendants die with it.
///
/// True where a parent-death mechanism exists: `PR_SET_PDEATHSIG` on Linux, a
/// job object on Windows. False on macOS, which has neither — so on macOS,
/// ending the guardian orphans whatever it started rather than reclaiming it.
pub const DESCENDANTS_DIE_WITH_THE_PROCESS: bool = cfg!(any(target_os = "linux", windows));

/// Arms the backend to die with the guardian, where the kernel can do it.
///
/// Linux only. macOS has no equivalent, and on Windows the job object in
/// [`contain_guardian`] is the mechanism instead.
pub fn arm_parent_death(command: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;

        // `command` is consumed by `spawn` before it can be borrowed again, so
        // this cannot be installed after the fork.
        let guardian = std::process::id();
        unsafe {
            command.pre_exec(move || {
                // Fires when the thread that forked this child goes away, and
                // the guardian only ever spawns from its single main thread.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The guardian may already have died between fork and here, in
                // which case the death signal never fires. Close that window.
                if libc::getppid() as u32 != guardian {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = command;
    }
}

/// CREATE_NO_WINDOW is not the same as having no console association. Start
/// detached so the guardian can positively create its OWN console with
/// AllocConsole; do not recover by attaching to/reusing an inherited console.
pub fn configure_guardian(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
        command.creation_flags(DETACHED_PROCESS);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Use the private guardian console, but give the backend its own control
/// group. CREATE_NO_WINDOW must not be combined with this: a console-less CLI
/// cannot receive the CTRL_BREAK_EVENT which its CancelKeyPress handler needs.
pub fn configure_backend(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(windows))]
    let _ = command;
}

// Keep the explicit nonzero guard testable without signalling any OS process.
#[cfg(any(windows, test))]
fn target_owned_group(pid: u32, signal: impl FnOnce(u32) -> bool) -> bool {
    pid != 0 && signal(pid)
}

/// Caller must own an unreaped child spawned by configure_backend, after a
/// private console was established. Never accepts a remembered or reported PID.
#[cfg(windows)]
pub fn interrupt_backend(pid: u32) -> bool {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
    target_owned_group(pid, |group| unsafe {
        GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, group) != 0
    })
}

/// Allocate only our own console, before any backend exists. Never attach to a
/// user's console. Restore ALL inherited standard handles so allocating the
/// console cannot replace the guardian's keepalive/event pipes.
#[cfg(windows)]
fn private_console() -> Result<(), String> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AllocConsole, GetConsoleCP, GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|kind| (kind, GetStdHandle(kind)));
        if saved
            .iter()
            .any(|(_, handle)| handle.is_null() || *handle == INVALID_HANDLE_VALUE)
        {
            return Err(
                "guardian standard pipes are unavailable; refusing to spawn backend".into(),
            );
        }
        if AllocConsole() == 0 {
            return Err(format!(
                "AllocConsole failed; refusing to reuse an existing console: {}; console code page={}",
                std::io::Error::last_os_error(),
                GetConsoleCP()
            ));
        }
        let window = GetConsoleWindow();
        if !window.is_null() {
            ShowWindow(window, SW_HIDE);
        }
        let mut failure = None;
        for (kind, handle) in saved {
            if SetStdHandle(kind, handle) == 0 {
                failure.get_or_insert_with(std::io::Error::last_os_error);
            }
        }
        if let Some(error) = failure {
            return Err(format!(
                "cannot restore guardian pipes after AllocConsole: {error}"
            ));
        }
    }
    Ok(())
}

/// Windows: puts **this process** in a job the kernel kills with it.
///
/// Called once at guardian startup, *before* the backend exists. That ordering
/// is the point: a Windows job is inherited by child processes, so the backend
/// — and anything the backend itself starts — joins the job automatically at
/// creation. There is no window in which the backend runs uncontained, and no
/// process is ever adopted into the job after it has had a chance to execute.
///
/// The handle is deliberately never closed on success: the kernel closes it as
/// this process dies, and closing it *is* the kill request
/// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) for everything in the job.
///
/// On failure the handle is closed and an error is returned, so the caller can
/// refuse to start the backend at all. Nothing has been spawned at that point,
/// so there is no created process to converge — the failure branch is
/// fail-closed by construction rather than by cleanup.
///
/// Elsewhere there is nothing to do and this returns `Ok(())`.
pub fn contain_guardian() -> Result<(), String> {
    #[cfg(windows)]
    {
        // Fail before spawn if the private control channel cannot be set up.
        private_console()?;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!(
                    "CreateJobObject 失败：{}",
                    std::io::Error::last_os_error()
                ));
            }

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(format!("SetInformationJobObject 失败：{error}"));
            }

            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(format!("AssignProcessToJobObject 失败：{error}"));
            }

            // Intentionally do not CloseHandle on success. This raw handle has
            // no Drop; the kernel closes it when the guardian exits, taking the
            // job down. mem::forget on this Copy value would do nothing.
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_broadcasts_a_console_control_event_to_group_zero() {
        let mut calls = 0;
        assert!(!target_owned_group(0, |_| {
            calls += 1;
            true
        }));
        assert_eq!(calls, 0);
    }

    #[test]
    fn addresses_only_the_owned_backend_group_and_keeps_failure_visible() {
        let mut addressed = Vec::new();
        assert!(target_owned_group(71, |group| {
            addressed.push(group);
            true
        }));
        assert!(!target_owned_group(72, |group| {
            addressed.push(group);
            false
        }));
        assert_eq!(addressed, [71, 72]);
    }
}
