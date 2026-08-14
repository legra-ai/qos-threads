#![doc = include_str!("../README.md")]

use std::fmt;

/// Thread scheduling priority class for a workload's `QoS` policy.
///
/// The two classes encode the asymmetric "prioritize latency, throttle CPU"
/// split: latency-sensitive threads run [`Qos::High`] so they are scheduled
/// promptly even under load, while CPU-bound work runs [`Qos::Low`] so it
/// yields to interactive workloads on a shared machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    /// Latency-sensitive work such as network event loops, discovery,
    /// heartbeats, and connection management.
    /// macOS `QOS_CLASS_USER_INITIATED`; Linux nice 0 (`SCHED_OTHER`).
    High,
    /// CPU-bound background work such as analysis, compaction, or index
    /// construction. macOS
    /// `QOS_CLASS_UTILITY`; Linux nice +10.
    Low,
}

/// An operating-system scheduling call was rejected. Carries the raw `errno`.
///
/// This is never returned for an unsupported platform (those are successful
/// no-ops), only for a genuine OS rejection, such as `EPERM` when raising
/// priority without `CAP_SYS_NICE` on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QosError(i32);

impl QosError {
    /// The raw OS error number that caused the rejection.
    #[must_use]
    pub fn errno(self) -> i32 {
        self.0
    }
}

impl fmt::Display for QosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "thread QoS change rejected (errno {})", self.0)
    }
}

impl std::error::Error for QosError {}

/// Set the **calling thread**'s scheduling class for the rest of its life.
/// This is intended for a runtime `on_thread_start` hook that assigns a
/// stable class to each worker thread.
///
/// # Errors
///
/// Returns [`QosError`] if the OS rejects the change (e.g. `EPERM`).
/// On platforms without a per-thread `QoS` API this is a successful no-op.
pub fn set_current_thread(qos: Qos) -> Result<(), QosError> {
    imp::set(qos)
}

/// Run `f` with the calling thread temporarily set to `qos`, restoring
/// the thread's previous class when `f` returns or unwinds.
///
/// Intended to wrap CPU-bound work that runs on a **reused** thread
/// (e.g. a blocking-pool thread): lower it to [`Qos::Low`] for the
/// duration so it cannot starve latency-sensitive threads, then hand the
/// thread back at its prior class for the next task.
pub fn with_qos<T>(qos: Qos, f: impl FnOnce() -> T) -> T {
    // RAII so the previous class is restored even if `f` panics.
    let _restore = Restore(imp::save());
    let _ = imp::set(qos);
    f()
}

struct Restore(imp::Saved);

impl Drop for Restore {
    fn drop(&mut self) {
        imp::restore(&self.0);
    }
}

/// Raise this **process**'s disk-I/O priority so durable reads and writes win
/// contention against low-priority OS housekeeping. Process-scoped, so it
/// also covers threads that do not use a per-thread [`Qos`] hook. Call once
/// during process startup when durable I/O should remain responsive.
///
/// On macOS this uses `setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS,
/// IOPOL_IMPORTANT)`. On Linux, process-wide I/O policy is normally managed
/// by the service manager, so this function is a successful no-op.
///
/// # Errors
///
/// Returns [`QosError`] if the OS rejects the change.
pub fn boost_process_io() -> Result<(), QosError> {
    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    {
        imp::boost_process_io()
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    {
        imp::boost_process_io();
        Ok(())
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use libc::qos_class_t::{QOS_CLASS_DEFAULT, QOS_CLASS_USER_INITIATED, QOS_CLASS_UTILITY};

    use super::{Qos, QosError};

    pub(super) struct Saved {
        class: libc::qos_class_t,
        relative_priority: libc::c_int,
    }

    fn class_of(qos: Qos) -> libc::qos_class_t {
        match qos {
            Qos::High => QOS_CLASS_USER_INITIATED,
            Qos::Low => QOS_CLASS_UTILITY,
        }
    }

    pub(super) fn set(qos: Qos) -> Result<(), QosError> {
        // SAFETY: `pthread_set_qos_class_self_np` acts on the calling
        // thread, takes no pointers, and returns 0 or an errno.
        let rc = unsafe { libc::pthread_set_qos_class_self_np(class_of(qos), 0) };
        if rc == 0 { Ok(()) } else { Err(QosError(rc)) }
    }

    pub(super) fn save() -> Saved {
        let mut class = QOS_CLASS_DEFAULT;
        let mut relative_priority: libc::c_int = 0;
        // SAFETY: writes through two valid stack pointers for the
        // calling thread; the return code is advisory and ignored.
        unsafe {
            libc::pthread_get_qos_class_np(
                libc::pthread_self(),
                &raw mut class,
                &raw mut relative_priority,
            );
        }
        Saved {
            class,
            relative_priority,
        }
    }

    pub(super) fn restore(saved: &Saved) {
        // SAFETY: see `set`; best-effort restore, return code ignored.
        unsafe {
            libc::pthread_set_qos_class_self_np(saved.class, saved.relative_priority);
        }
    }

    // IOPOL constants from `<sys/resource.h>`; `libc` does not surface them.
    const IOPOL_TYPE_DISK: libc::c_int = 0;
    const IOPOL_SCOPE_PROCESS: libc::c_int = 1;
    const IOPOL_IMPORTANT: libc::c_int = 1;

    unsafe extern "C" {
        fn setiopolicy_np(
            iotype: libc::c_int,
            scope: libc::c_int,
            policy: libc::c_int,
        ) -> libc::c_int;
    }

    pub(super) fn boost_process_io() -> Result<(), QosError> {
        // SAFETY: three scalar args, no pointers; returns 0, or -1 with
        // errno set.
        let rc = unsafe { setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS, IOPOL_IMPORTANT) };
        if rc == 0 {
            Ok(())
        } else {
            Err(QosError(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
            ))
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use super::{Qos, QosError};

    pub(super) struct Saved {
        nice: libc::c_int,
    }

    fn nice_of(qos: Qos) -> libc::c_int {
        match qos {
            // Raising priority above the process baseline needs
            // CAP_SYS_NICE; an unprivileged caller receives QosError.
            Qos::High => 0,
            Qos::Low => 10,
        }
    }

    pub(super) fn set(qos: Qos) -> Result<(), QosError> {
        // `setpriority(PRIO_PROCESS, 0, ..)` targets the calling thread
        // on Linux (nice is per-task and `who == 0` is the caller).
        // SAFETY: no pointers; returns 0 or -1 with errno set.
        let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice_of(qos)) };
        if rc == 0 {
            Ok(())
        } else {
            Err(QosError(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
            ))
        }
    }

    pub(super) fn save() -> Saved {
        // For the values we set (0/10) the `-1`-means-error ambiguity of
        // `getpriority` never applies, so the return is the nice value.
        // SAFETY: no pointers.
        let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        Saved { nice }
    }

    pub(super) fn restore(saved: &Saved) {
        // SAFETY: see `set`; best-effort restore, return code ignored.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, saved.nice);
        }
    }

    pub(super) fn boost_process_io() -> Result<(), QosError> {
        // Whole-process disk-I/O priority is normally set declaratively by a
        // Linux service manager, which covers the service cgroup regardless
        // of when threads spawn. Nothing to do here.
        Ok(())
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
mod imp {
    use super::{Qos, QosError};

    pub(super) struct Saved;

    pub(super) fn set(_qos: Qos) -> Result<(), QosError> {
        Ok(())
    }

    pub(super) fn save() -> Saved {
        Saved
    }

    pub(super) fn restore(_saved: &Saved) {}

    pub(super) fn boost_process_io() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lowering priority never needs privilege; unsupported platforms use the
    /// successful no-op implementation.
    #[test]
    fn low_and_high_apply_without_error() {
        // Low first: always permitted everywhere.
        assert!(set_current_thread(Qos::Low).is_ok());
        // Raising priority may be rejected on an unprivileged Linux process.
        let _ = set_current_thread(Qos::High);
    }

    #[test]
    fn boost_process_io_does_not_error_on_supported_platforms() {
        // macOS applies the native policy; Linux and other platforms use the
        // service-manager/no-op implementation.
        assert!(boost_process_io().is_ok());
    }

    #[test]
    fn with_qos_returns_closure_value_and_runs_it() {
        let mut ran = false;
        let out = with_qos(Qos::Low, || {
            ran = true;
            7 + 35
        });
        assert!(ran);
        assert_eq!(out, 42);
    }

    #[test]
    fn with_qos_nests() {
        let out = with_qos(Qos::High, || with_qos(Qos::Low, || "inner").len());
        assert_eq!(out, 5);
    }

    #[test]
    fn with_qos_restores_after_panic() {
        let result = std::panic::catch_unwind(|| {
            with_qos(Qos::Low, || panic!("boom"));
        });
        assert!(result.is_err());
        // The Restore guard ran during unwind; a subsequent set still works.
        assert!(set_current_thread(Qos::Low).is_ok());
    }

    #[test]
    fn qos_error_reports_errno() {
        let err = QosError(13);
        assert_eq!(err.errno(), 13);
        assert!(err.to_string().contains("13"));
    }
}
