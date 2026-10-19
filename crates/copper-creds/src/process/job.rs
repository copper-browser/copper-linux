//! A job object around a vault CLI so its whole process tree ends with the
//! call.
//!
//! `bw` from npm is `bw.cmd`: `cmd.exe` starting `node.exe`. Killing the
//! child we spawned (on timeout, `kill_on_drop`) ends `cmd.exe` only and
//! would leave `node` running. With `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
//! closing the job's handle ends every process still in it — on timeout and
//! after a normal exit alike, so nothing a CLI started outlives the call.
//! Jobs nest (Windows 8+), so this works when Copper itself runs in one.
//!
//! The child is assigned right after it starts; anything it starts in the
//! first instant before that escapes the job. Neither CLI does.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::core::PCWSTR;

/// Owns the job; dropping it ends the processes inside.
pub(crate) struct KillTree(HANDLE);

// SAFETY: a job handle is a kernel object handle, valid from any thread;
// `KillTree` only closes it. Needed because `execute`'s future holds it
// across awaits and runs on a multi-threaded runtime.
unsafe impl Send for KillTree {}
unsafe impl Sync for KillTree {}

impl KillTree {
    /// Put `child` in a fresh kill-on-close job. `None` (logged) when
    /// Windows refuses: the call then falls back to killing the child alone.
    pub(crate) fn adopt(child: &tokio::process::Child) -> Option<Self> {
        let process = HANDLE(child.raw_handle()?);
        // SAFETY: a new unnamed job with default security; the handle is
        // owned by `KillTree` from here and closed exactly once in Drop.
        let job = match unsafe { CreateJobObjectW(None, PCWSTR::null()) } {
            Ok(job) => KillTree(job),
            Err(e) => {
                tracing::debug!("vault CLI: no job object: {e}");
                return None;
            }
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the struct this information class names, with
        // its exact size; `process` is the live child's handle (the child is
        // borrowed, so not yet reaped).
        let set = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .and_then(|()| AssignProcessToJobObject(job.0, process))
        };
        match set {
            Ok(()) => Some(job),
            Err(e) => {
                tracing::debug!("vault CLI: could not join a job object: {e}");
                None
            }
        }
    }
}

impl Drop for KillTree {
    fn drop(&mut self) {
        // SAFETY: the job handle this value owns, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
