//! Platform process-exit boundary.
//!
//! `std::process::ExitCode` intentionally stores only an eight-bit portable
//! status. XUVA is a Windows command dispatcher, so child statuses must retain
//! the complete signed 32-bit value returned by `ExitStatus::code()`.

use std::process::ExitStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CliExit(i32);

impl CliExit {
    pub(crate) const SUCCESS: Self = Self(0);
    pub(crate) const FAILURE: Self = Self(1);

    pub(crate) fn from_status(status: ExitStatus) -> Self {
        Self(status.code().unwrap_or(1))
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn terminate(self) -> ! {
        use std::io::Write;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn ExitProcess(exit_code: u32) -> !;
        }

        // ExitProcess bypasses Rust's normal runtime teardown, so explicitly
        // flush dispatcher-owned buffered output before crossing that boundary.
        // Child processes write to the inherited handles directly, but local
        // commands such as `agent hook` and `--version` use Rust's writers.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        // SAFETY: ExitProcess accepts the complete 32-bit process status and
        // never returns. The cast preserves its native bit pattern.
        unsafe { ExitProcess(self.0 as u32) }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn terminate(self) -> ! {
        std::process::exit(self.0)
    }
}

impl From<i32> for CliExit {
    fn from(value: i32) -> Self {
        Self(value)
    }
}
