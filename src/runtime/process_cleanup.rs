use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessCleanupReason {
    Timeout,
    Terminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ProcessCleanupReport {
    pub root_pid: Option<u32>,
    pub process_group_id: Option<i32>,
    pub graceful_signal_sent: bool,
    pub force_kill_sent: bool,
    pub direct_child_kill_sent: bool,
    pub success: bool,
}

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut tokio::process::Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub(crate) fn configure_process_group(_command: &mut tokio::process::Command) {}

#[cfg(unix)]
pub(crate) async fn cleanup_child_process_tree(
    child: &mut tokio::process::Child,
    _reason: ProcessCleanupReason,
    grace: Duration,
) -> ProcessCleanupReport {
    let root_pid = child.id();
    let process_group_id = root_pid.and_then(|pid| {
        let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
        (pgid > 0).then_some(pgid)
    });

    let mut graceful_signal_sent = false;
    let mut force_kill_sent = false;
    let mut direct_child_kill_sent = false;

    if let Some(pgid) = process_group_id {
        graceful_signal_sent = unsafe { libc::killpg(pgid, libc::SIGTERM) } == 0;
        if !wait_until_exited(child, grace).await {
            force_kill_sent = unsafe { libc::killpg(pgid, libc::SIGKILL) } == 0;
        }
    }

    if child.try_wait().ok().flatten().is_none() {
        direct_child_kill_sent = child.start_kill().is_ok();
    }

    let success = child.wait().await.is_ok();

    ProcessCleanupReport {
        root_pid,
        process_group_id,
        graceful_signal_sent,
        force_kill_sent,
        direct_child_kill_sent,
        success,
    }
}

#[cfg(unix)]
async fn wait_until_exited(child: &mut tokio::process::Child, grace: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }

        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(not(unix))]
pub(crate) async fn cleanup_child_process_tree(
    child: &mut tokio::process::Child,
    _reason: ProcessCleanupReason,
    _grace: Duration,
) -> ProcessCleanupReport {
    let root_pid = child.id();
    let direct_child_kill_sent = child.start_kill().is_ok();
    let success = child.wait().await.is_ok();

    ProcessCleanupReport {
        root_pid,
        process_group_id: None,
        graceful_signal_sent: false,
        force_kill_sent: false,
        direct_child_kill_sent,
        success,
    }
}
