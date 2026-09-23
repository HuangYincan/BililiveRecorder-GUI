//! The places where the guardian's guarantees need the operating system.
//!
//! Everything here is kept in one module that depends on nothing but `std` and
//! raw bindings, so that it can be type-checked for every target even though
//! the crate as a whole cannot be: `cargo check` needs no linker, but tauri's
//! C dependencies need a real sysroot for the target (`windows.h`, GTK), so
//! cross-checking the whole crate fails before it ever reaches Rust code. This
//! module has no such dependency and *is* checked for Linux and Windows.
//!
//! Being type-checked is not being tested. Nothing in this file has been run on
//! Windows or Linux; see the platform support boundary in `lib.rs`.

use std::process::{Child, Command};

/// Pids the kernel reports as *direct children* of `pid`.
///
/// This is the kernel's own parent/child relation. It is consulted only when a
/// guardian is alive but not making progress, and it is never used to decide
/// whether a pid is ours: the caller holds the guardian as an unreaped `Child`,
/// so the pid is still reserved, and a guardian that has been reaped has no
/// children left for this to report.
pub fn kernel_children_of(pid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
            Ok(text) => text
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0 as libc::c_int; 64];
        // Returns how many pids were written, or a negative value on failure.
        let written = unsafe {
            libc::proc_listchildpids(
                pid as libc::c_int,
                buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(&buffer) as libc::c_int,
            )
        };
        if written <= 0 {
            return Vec::new();
        }
        let count = (written as usize).min(buffer.len());
        buffer[..count]
            .iter()
            .map(|raw| *raw as u32)
            .filter(|child| *child != 0)
            .collect()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Windows needs none of this: killing the guardian closes its job,
        // which the kernel has already tied to the backend's lifetime.
        let _ = pid;
        Vec::new()
    }
}

/// Arms the backend to die with the guardian, where the kernel can do it.
///
/// Linux only. macOS has no equivalent, and on Windows the job object in
/// [`adopt_backend_into_job`] is the mechanism instead.
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

/// Windows: puts the backend in a job the kernel kills with this process.
///
/// The handle is deliberately never closed — the kernel closes it as this
/// process dies, and closing it *is* the kill request
/// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). That is what makes an outright
/// killed guardian still take the backend with it.
///
/// Elsewhere there is nothing to do and this returns `Ok(())`.
pub fn adopt_backend_into_job(child: &Child) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err("CreateJobObject 失败".into());
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
                return Err("SetInformationJobObject 失败".into());
            }

            if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                return Err("AssignProcessToJobObject 失败".into());
            }

            // Intentionally leaked: the handle must outlive the backend.
            std::mem::forget(job);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = child;
        Ok(())
    }
}
