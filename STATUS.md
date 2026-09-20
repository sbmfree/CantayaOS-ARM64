# CantayaOS Status

**Project:** CantayaOS by CantayaTech
**Target:** AArch64, UEFI, QEMU `virt` with OVMF
**Last verified:** 2026-09-20

## Current Milestone

**Objective:** Maintain verified lifecycle controls, including process-wide
termination of ready and typed-completion-blocked sibling threads.

## Verified Baseline For Planning

- The Rust UEFI loader starts the Rust AArch64 kernel and supplies a validated
  boot handoff containing framebuffer, memory-map, kernel, and initial-image
  information.
- Kernel execution and exception vectors run through the TTBR1 high-half
  mapping; every EL0 process owns an isolated TTBR0 root.
- The timer-driven scheduler resumes independently mapped EL0 workloads, and
  process/thread handles retain completion state safely across deferred thread
  reaping.
- The two boot-loaded `init.elf` processes remain a context-switch validation
  workload.
- Current EL0 coverage includes validated user-memory copy-in/copy-out, virtual
  allocation and free, thread create/wait/close/terminate, process
  create/wait/close/terminate, and system-information query.
- `NtTerminateThread` accepts only a parent-owned typed thread handle. It
  rejects the current thread, removes a non-current queued target safely,
  signals its handle completion with the requested status, and treats an
  already-complete target as a successful no-op.
- Typed handle-table entries enforce distinct `WAIT` and `TERMINATE` rights
  before revealing their retained process or thread object. Current process
  and thread handles receive both lifecycle rights; denied rights return
  `STATUS_ACCESS_DENIED` without escaping the table.
- `NtWaitForSingleObject` accepts a validated relative timeout of one to 1,000
  scheduler ticks through its existing timeout pointer. Expiration returns
  `STATUS_TIMEOUT`, never writes a completion status, and removes the waiter
  from the typed completion object before it can be signaled later.
- The scheduler tracks typed process and thread completion waiters separately
  from ready threads. Wake, timeout, thread termination, and process
  termination remove that tracking before a raw thread record can be reaped.
- `NtCreateProcess` can load only the retained boot-validated `init.elf` or the
  fixed, kernel-owned live VirtIO/FAT `CHILD.ELF` source. Both follow the same
  ELF and address-space validation path.

Detailed guarantees and validation evidence live in
[docs/verified-features.md](docs/verified-features.md). Stable subsystem design
lives in [docs/architecture.md](docs/architecture.md).

## Recommended Next Milestone

Prove `NtTerminateProcess(process_handle, status)` for an externally targeted
process that has a sibling blocked in a typed finite or infinite wait. Preserve
the existing scheduler tracking, cancel the sibling's completion registration
before deferred reaping, and observe the final process status through the
existing parent process handle. Do not add process groups, job objects,
cross-process thread creation, or new lifecycle syscalls.

## Hard Constraints And Do Not Implement Yet

- The low device identity mapping remains only for bootstrap and kernel-mode
  execution; it is not copied into any user TTBR0 root. Each user root retains
  only the supervisor-only `0x4000_0000` kernel-image alias required until the
  kernel is linked fully in the high half. EL1 support paths otherwise use
  high-half mappings.
- `NtCreateProcess` accepts only selector zero for the kernel-owned copy of the
  boot-validated image or selector one for the I/O-owned, bounded live VirtIO
  FAT `CHILD.ELF` object. Do not add arbitrary file names, raw user ELF input,
  inherited handles, command lines, or security tokens. It returns a
  parent-owned process handle and accepts a controlled initial EL0 `x1` value.
- `NtCreateThread` is limited to the current process and requires an already
  mapped executable entry point and writable stack. Do not add suspended
  creation, priorities, APCs, or cross-process thread creation.
- `NtWaitForSingleObject` supports non-alertable infinite waits, or one to
  1,000 relative 100 Hz scheduler ticks through its existing timeout pointer,
  with an optional fourth user pointer for the final `i32` target status. A
  timeout returns `STATUS_TIMEOUT` without writing that output. Do not add wait
  sets, alertable waits, APCs, or generic timeout objects.
- `NtTerminateProcess(-1, status)` is process-wide: it removes every ready
  sibling and every tracked typed-completion waiter in the caller's process,
  cancels their registrations, signals each thread with the requested status,
  then terminates the caller so the process completes with that same status.
  Raw thread records remain deferred until another context is active.
  `NtTerminateProcess(process_handle, status)` retains its existing external
  target behavior. Do not add process groups, job objects, or cross-process
  thread creation. `NtTerminateThread(thread_handle, status)` accepts only a
  parent-owned typed thread handle; it rejects a target that is `CURRENT`,
  removes a ready target or tracked typed-completion waiter, signals the
  requested final status, and defers raw-stack reclamation. A repeated request
  after completion succeeds without replacing the status.
- `NtAllocateVirtual` accepts only the current-process pseudo-handle. It has
  automatic first-fit reuse and page-aligned fixed placement in the dynamic
  user-memory window, but no reservation/commit split or protection changes.
- The only completed I/O request is a synchronous kernel-owned read by the
  fixed `CHILD.ELF` file object. Do not add EL0 filenames, arbitrary devices or
  offsets, pending I/O, or cancellation.
- Most NT syscall entries, I/O facilities, object access checks, and the
  security model remain scaffolding rather than finished operating-system
  services.
- Remaining lifecycle work must retain the verified cancellation of blocked
  typed waits while proving the external process-termination path.
- Security tokens/access checks, SMP, per-CPU scheduling, and GICv3 are
  deferred after lifecycle controls.

## Current Blockers

None recorded for the current multi-thread process-exit milestone.

## Verification Requirements

Run `make smoke` for meaningful kernel, MMU, scheduler, syscall, process, or
I/O changes. It is the regression check for the QEMU `virt`/TCG path and must
demonstrate the required runtime markers; a successful compile alone is not
completion. This is not hardware certification or evidence of Windows
application compatibility.

## Detailed References

- [Documentation map](docs/README.md)
- [Stable architecture](docs/architecture.md)
- [Verified features and smoke-test evidence](docs/verified-features.md)
