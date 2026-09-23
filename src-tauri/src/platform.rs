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
//! Nothing here ever signals a process. Every signal in this program is sent to
//! a [`std::process::Child`] the program spawned itself and has not reaped, so
//! the kernel still reserves that pid. This module's job is only to make the
//! *kernel* enforce that a process's descendants die with it, which is what lets
//! the guardian's own death be enough.

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

            // Intentionally leaked: the kernel must close this handle, because
            // doing so is what takes the backend down.
            std::mem::forget(job);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        Ok(())
    }
}
