# qos-threads

[![Crates.io](https://img.shields.io/crates/v/qos-threads.svg)](https://crates.io/crates/qos-threads)
[![Documentation](https://docs.rs/qos-threads/badge.svg)](https://docs.rs/qos-threads)
[![CI/CD](https://github.com/legra-ai/qos-threads/actions/workflows/ci.yml/badge.svg)](https://github.com/legra-ai/qos-threads/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/crates/l/qos-threads.svg)](https://github.com/legra-ai/qos-threads#license)
[![Downloads](https://img.shields.io/crates/d/qos-threads.svg)](https://crates.io/crates/qos-threads)

Cross-platform **Quality-of-Service controls for native threads and
processes**.

This crate provides a small platform abstraction for workloads that share a
machine but have different scheduling needs:

- `Qos::High` — latency-sensitive work that should remain responsive under
  load. On macOS this maps to `QOS_CLASS_USER_INITIATED`; on Linux it maps to
  the normal nice level.
- `Qos::Low` — background or CPU-bound work that should yield to interactive
  workloads. On macOS this maps to `QOS_CLASS_UTILITY`; on Linux it maps to a
  positive nice level.

## Thread controls

Use `set_current_thread` from a thread-pool startup hook when a thread keeps
the same scheduling class for its lifetime:

```rust
use qos_threads::{Qos, set_current_thread};

set_current_thread(Qos::Low)?;
# Ok::<(), qos_threads::QosError>(())
```

Use `with_qos` when a reused thread needs a temporary class. The previous
class is restored even when the closure unwinds:

```rust
use qos_threads::{Qos, with_qos};

let answer = with_qos(Qos::Low, || 20 + 22);
assert_eq!(answer, 42);
```

## Process I/O

`boost_process_io` raises the process's disk-I/O policy where the operating
system provides such a control. This is useful when durable I/O should not be
treated as background housekeeping. On platforms where the process-level
policy is managed by the service manager, the function is a successful no-op.

## Platform behavior

The API is intentionally small and platform-neutral. Unsupported platforms
return success without changing scheduling, while an operating-system
rejection is returned as `QosError`. Callers can decide whether a scheduling
policy is advisory or mandatory for their workload.

The crate currently implements native controls for Apple platforms and Linux
or Android. Other platforms compile with the documented no-op behavior.

## License

Licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.
