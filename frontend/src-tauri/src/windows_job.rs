//! Kill-on-close job for a native helper owned by this process.

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    },
};

pub(crate) struct Job(HANDLE);
// The owned handle is thread-independent, and assignment is thread-safe.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub(crate) fn new() -> windows::core::Result<Self> {
        // SAFETY: the job is unnamed and non-inheritable with default security.
        let job = Self(unsafe { CreateJobObjectW(None, PCWSTR::null()) }?);
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the job is valid and the corresponding structure lives
        // through this call.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        }?;
        Ok(job)
    }

    pub(crate) fn assign(
        &self,
        process: std::os::windows::io::BorrowedHandle<'_>,
    ) -> windows::core::Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: both handles belong to live Rust owners during the call.
        unsafe { AssignProcessToJobObject(self.0, HANDLE(process.as_raw_handle() as isize)) }
    }
}

#[cfg(test)]
#[tokio::test]
async fn closing_the_job_terminates_its_helper() {
    use std::os::windows::io::BorrowedHandle;
    use std::time::Duration;

    let job = Job::new().unwrap();
    let mut child = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 60",
        ])
        .creation_flags(0x08000000)
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let handle = child.raw_handle().unwrap();
    // SAFETY: the test exclusively owns this live child and its handle.
    job.assign(unsafe { BorrowedHandle::borrow_raw(handle) })
        .unwrap();
    assert!(child.try_wait().unwrap().is_none());
    drop(job);
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: this is the owned job handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
