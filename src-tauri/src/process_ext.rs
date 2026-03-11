//! Cross-platform process extensions for Windows console suppression and
//! child-process lifecycle management.
//!
//! - `HideConsole` trait: suppresses the transient console window that Windows
//!   creates for every `Command::new()` spawn (CREATE_NO_WINDOW flag).
//!   No-op on non-Windows.
//!
//! - `setup_job_kill_on_close()`: creates a Windows Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so child processes are automatically
//!   terminated when the main process exits (including crash / force-kill).
//!   No-op on non-Windows.

/// Extension trait to suppress console-window flashing on Windows.
///
/// On Windows, every `Command::new()` spawn opens a visible console window
/// unless `CREATE_NO_WINDOW` (0x0800_0000) is set via `creation_flags()`.
/// This trait provides a cross-platform `.hide_console()` builder method.
pub trait HideConsole {
    fn hide_console(&mut self) -> &mut Self;
}

// ── Windows implementation ──────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::HideConsole;
    use std::os::windows::process::CommandExt;

    /// `CREATE_NO_WINDOW` — prevents the OS from creating a visible console
    /// window for the child process. Safe for GUI apps that capture stdio.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    impl HideConsole for std::process::Command {
        fn hide_console(&mut self) -> &mut Self {
            self.creation_flags(CREATE_NO_WINDOW)
        }
    }

    impl HideConsole for tokio::process::Command {
        fn hide_console(&mut self) -> &mut Self {
            self.creation_flags(CREATE_NO_WINDOW)
        }
    }
}

// ── Non-Windows no-op ───────────────────────────────────────────────────────

#[cfg(not(windows))]
mod imp {
    use super::HideConsole;

    impl HideConsole for std::process::Command {
        fn hide_console(&mut self) -> &mut Self {
            self // no-op
        }
    }

    impl HideConsole for tokio::process::Command {
        fn hide_console(&mut self) -> &mut Self {
            self // no-op
        }
    }
}

// Re-export so `use crate::process_ext::HideConsole;` works everywhere.
#[allow(unused_imports)]
pub use imp::*;

// ── Windows Job Object: kill children on process exit ───────────────────────

/// Set up a Windows Job Object so that all child processes are automatically
/// killed when the main process exits (including crash or force-kill via Task Manager).
///
/// On non-Windows this is a no-op.
///
/// # How it works
///
/// 1. `CreateJobObjectW` — create an anonymous Job Object.
/// 2. `SetInformationJobObject` — set `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so
///    Windows kills all processes in the Job when the last handle is closed.
/// 3. `AssignProcessToJobObject` — add the current process to the Job.
/// 4. Intentionally never close the Job handle — it stays open until
///    process exit, at which point the OS closes it and kills children.
///
/// # Failure modes (all non-panic)
///
/// - `CreateJobObjectW` fails → log warning, return.
/// - `SetInformationJobObject` fails → `CloseHandle`, log warning, return.
/// - `AssignProcessToJobObject` fails with `ERROR_ACCESS_DENIED` (5) → the
///   process is already in another Job (e.g. launched from Terminal, IDE).
///   Windows 8+ supports nested Jobs, but some environments still deny this.
///   We log a warning noting that crash/force-kill cleanup is best-effort.
/// - Any other `AssignProcessToJobObject` error → `CloseHandle`, log warning.
///
/// # Safety notes
///
/// - `extern "system"` functions auto-link to kernel32.dll on Windows.
/// - The `#[repr(C)]` structs match the Windows SDK `winnt.h` layout.
/// - Tauri v2 requires Windows 10+, so nested Job Objects are supported.
#[cfg(not(windows))]
pub fn setup_job_kill_on_close() {
    // no-op on non-Windows
}

#[cfg(windows)]
pub fn setup_job_kill_on_close() {
    use std::ffi::c_void;
    use std::mem;
    use std::ptr;

    // ── Win32 constants ─────────────────────────────────────────────────

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// `JobObjectExtendedLimitInformation` information class = 9.
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[allow(clippy::upper_case_acronyms)]
    type HANDLE = *mut c_void;
    #[allow(clippy::upper_case_acronyms)]
    type BOOL = i32;
    #[allow(clippy::upper_case_acronyms)]
    type DWORD = u32;
    #[allow(clippy::upper_case_acronyms)]
    type LPCWSTR = *const u16;

    // ── Win32 structs (layout from Windows SDK winnt.h) ─────────────────

    #[repr(C)]
    #[derive(Default)]
    #[allow(non_snake_case)]
    struct IO_COUNTERS {
        ReadOperationCount: u64,
        WriteOperationCount: u64,
        OtherOperationCount: u64,
        ReadTransferCount: u64,
        WriteTransferCount: u64,
        OtherTransferCount: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    #[allow(non_snake_case)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        PerProcessUserTimeLimit: i64,
        PerJobUserTimeLimit: i64,
        LimitFlags: DWORD,
        MinimumWorkingSetSize: usize,
        MaximumWorkingSetSize: usize,
        ActiveProcessLimit: DWORD,
        Affinity: usize,
        PriorityClass: DWORD,
        SchedulingClass: DWORD,
    }

    #[repr(C)]
    #[derive(Default)]
    #[allow(non_snake_case)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION,
        IoInfo: IO_COUNTERS,
        ProcessMemoryLimit: usize,
        JobMemoryLimit: usize,
        PeakProcessMemoryUsed: usize,
        PeakJobMemoryUsed: usize,
    }

    // ── Win32 FFI declarations (auto-linked to kernel32.dll) ────────────

    extern "system" {
        fn CreateJobObjectW(lpJobAttributes: *mut c_void, lpName: LPCWSTR) -> HANDLE;
        fn SetInformationJobObject(
            hJob: HANDLE,
            JobObjectInformationClass: DWORD,
            lpJobObjectInformation: *const c_void,
            cbJobObjectInformationLength: DWORD,
        ) -> BOOL;
        fn AssignProcessToJobObject(hJob: HANDLE, hProcess: HANDLE) -> BOOL;
        fn GetCurrentProcess() -> HANDLE;
        fn CloseHandle(hObject: HANDLE) -> BOOL;
        fn GetLastError() -> DWORD;
    }

    // ── Step 1: Create anonymous Job Object ─────────────────────────────

    let job: HANDLE = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
    if job.is_null() {
        log::warn!(
            "[process_ext] CreateJobObjectW failed (err={}), child cleanup on crash not available",
            unsafe { GetLastError() }
        );
        return;
    }

    // ── Step 2: Set KILL_ON_JOB_CLOSE limit ─────────────────────────────

    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

    let ok = unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            &info as *const _ as *const c_void,
            mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as DWORD,
        )
    };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        unsafe { CloseHandle(job) };
        log::warn!(
            "[process_ext] SetInformationJobObject failed (err={}), child cleanup on crash not available",
            err
        );
        return;
    }

    // ── Step 3: Assign current process to the Job ───────────────────────

    let current = unsafe { GetCurrentProcess() };
    let ok = unsafe { AssignProcessToJobObject(job, current) };
    if ok == 0 {
        // IMPORTANT: GetLastError BEFORE CloseHandle — CloseHandle resets last error.
        let err = unsafe { GetLastError() };
        unsafe { CloseHandle(job) };

        if err == ERROR_ACCESS_DENIED {
            // Process is already in another Job (Terminal, IDE, CI sandbox).
            // Windows 8+ supports nested Jobs, but some environments still deny.
            // This is an acceptable path — graceful shutdown still works,
            // but crash/force-kill may not clean up children.
            log::warn!(
                "[process_ext] AssignProcessToJobObject: ACCESS_DENIED — \
                 process already in another Job. Graceful shutdown will still \
                 clean up children, but crash/force-kill cleanup is not guaranteed."
            );
        } else {
            log::warn!(
                "[process_ext] AssignProcessToJobObject failed (err={}), \
                 child cleanup on crash not available",
                err
            );
        }
        return;
    }

    // ── Step 4: Keep handle open until process exit ────────────────────
    // HANDLE is a raw pointer (*mut c_void) — it is Copy, so there is no
    // Drop to run.  Simply not calling CloseHandle(job) keeps the kernel
    // handle alive for the lifetime of the process.  When the process
    // exits, the OS closes the handle and kills all children in the Job.
    // (Do NOT call mem::forget here — it's a no-op on Copy types and
    // triggers clippy::forgetting_copy_types.)
    let _ = job; // suppress unused-variable warning, intentionally leaked

    log::debug!("[process_ext] Job Object created: children will be killed on process exit");
}
