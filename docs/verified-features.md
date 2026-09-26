# Verified Features

This is the detailed verified baseline for CantayaOS. It records completed
behavior and technical guarantees, not the current roadmap. For what to change
next, read [STATUS.md](../STATUS.md).

## Boot, Hardware, And Kernel Foundation

- The UEFI bootloader loads `kernel.elf` and `init.elf`, exits boot services,
  and passes framebuffer, memory-map, kernel, and init-image information to
  the kernel.
- AArch64 startup includes UART, framebuffer, GICv2, generic timer, exception
  vectors, typed ESR/FAR fault decoding, and a physical page allocator.
- `EPROCESS`, `ETHREAD`, scheduler, object, I/O, security, and kernel-base
  subsystem foundations exist with an NT-like structure.

## Memory Management And Address-Space Isolation

- The MMU uses a TTBR0 bootstrap mapping plus a TTBR1 alias at
  `0xffff800000000000`; kernel execution and exception vectors run through the
  high-half alias.
- TTBR1 includes a direct physical-memory window for the first 16 GiB. Kernel
  stacks, dynamic page tables, ELF backing pages, UART, GIC, framebuffer, and
  keyboard MMIO use high-half addresses through this window.
- Kernel virtual allocation creates and removes real high-half page mappings.
- User address spaces support explicit page mappings, owned-page cleanup, an
  unmapped stack guard page, W^X ELF segment validation, and instruction-cache
  synchronization after loading executable memory.
- User TTBR0 roots contain only deliberate EL0 mappings plus a supervisor-only
  alias of the still low-linked kernel image. The boot-time probe confirms an
  isolated root does not inherit the UART identity mapping.
- User address spaces provide all-or-nothing validated copy-in/copy-out and
  automatic read/write user-region allocation.

## EL0 Process, Thread, And Scheduler Behavior

- The initial EL0 program enters through `eret`, receives a guarded user stack,
  calls `NtWriteFile`, allocates and releases one page with
  `NtAllocateVirtual` and `NtFreeVirtual`, queries system information, and
  exits with `NtTerminateProcess`.
- Exception frames preserve all EL0 general-purpose registers, `ELR_EL1`, and
  `SPSR_EL1`; thread contexts preserve `SP_EL0`. The smoke workload runs two
  processes with separate address spaces and stacks to validate timer-driven
  EL0 resume.
- `NtCreateThread` validates its output pointer, user entry point, and user
  stack before creating an EL0 sibling thread. It returns a typed thread
  handle; `NtWaitForSingleObject` supports a non-alertable infinite wait and
  `NtClose` releases the handle after the wait completes.
- `NtTerminateThread` accepts only a parent-owned typed thread handle. It
  rejects `CURRENT`, safely removes a ready or waiting non-current target from
  the single-core queue, signals its requested final status, and defers raw
  stack reclamation. A repeated request after completion succeeds without
  replacing the original status. The boot-time scheduler checks current,
  queued, and completed targets; both EL0 init processes also terminate
  siblings blocked on typed thread and process completions with infinite and
  finite waits. Each finite waiter is reaped before its original target passes
  the former deadline, and process waiters are reaped before their controlled
  child process completes. Retained handles report requested statuses without
  stale wakeups from completion or timeout tracking.
- Terminated threads are deferred to the next active context before their
  `ETHREAD`, kernel stack, and final `EPROCESS` reference are destroyed. The
  process address space then releases its owned user pages and page tables.
- Handle tables retain typed `EPROCESS` and Arc-owned thread-completion
  objects. `WAIT` and `TERMINATE` are distinct table-local rights, checked
  before cloning an object reference; missing rights return
  `STATUS_ACCESS_DENIED`. Issued process and thread handles retain both rights
  needed by the current lifecycle callers. A process is signaled only after its
  final active thread exits; waiters are then readied without freeing a running
  thread's stack.
- `NtCreateProcess` creates a fresh address space from a kernel-owned copy of
  the boot-validated `init.elf`. It validates a parent output pointer, returns
  a typed process handle only after copy-out succeeds, and then enqueues the
  child's initial EL0 thread. It deliberately does not accept an arbitrary
  caller-provided ELF pointer.
- `NtTerminateProcess` accepts a parent-owned process handle as well as the
  current-process pseudo-handle. External termination removes all non-current
  ready target threads and typed-completion-blocked target threads from their
  scheduler-owned sets, clears every selected typed-completion registration
  before signaling any target completion, and defers stack reclamation until a
  different context is active.
- `NtTerminateProcess(-1, status)` follows the same bounded process-wide
  policy for the caller: ready and typed-completion-blocked same-process
  siblings are removed, their timer and retained typed-completion wait
  registrations are cancelled, and every thread receives `status`. The caller
  completes last, so the retained process object signals that same final status
  only after all active-thread accounting is complete. All raw thread records
  remain deferred until another context is active.
- Each init process launches a selector-zero child in its existing controlled
  startup mode. The child creates a spinning target and a sibling that blocks
  indefinitely on the target's typed thread handle. Its finite wait on that
  sibling yields enough scheduler ticks for the completion registration to be
  established before it invokes `NtTerminateProcess(-1, 0x61)`. The scheduler
  reports one cleared typed wait registration; the parent observes `0x61`
  through the returned typed process handle, closes it, and emits
  `[user-init] process-wide blocked wait termination validated`.
- Each init process also launches the controlled child in external-target mode.
  The parent waits ten scheduler ticks through the child's process handle, then
  calls `NtTerminateProcess(handle, 0x62)`, waits again, and observes `0x62`.
  The child has the same indefinitely blocked sibling; depending on scheduling,
  its finite setup wait can also still be registered, so the kernel reports one
  or more cleared typed wait registrations before reaping. The parent emits
  `[user-init] external process blocked wait termination validated`.
- Non-SVC synchronous exceptions from EL0 terminate only the faulting thread
  through the scheduler lifecycle path. Abort faults map to
  `STATUS_ACCESS_VIOLATION`, or `STATUS_STACK_OVERFLOW` for the mapped stack
  guard; alignment, breakpoint, and other synchronous classes map to stable
  NTSTATUS-style values. EL1 exceptions remain fatal.
- `NtWaitForSingleObject` retains its non-alertable infinite wait contract and
  accepts an optional validated fourth argument pointing to an `i32`
  completion-status output. Its existing timeout pointer may instead reference
  one through 1,000 relative scheduler ticks; timeout returns `STATUS_TIMEOUT`
  without changing the output. The timer path expires ordered deadlines and
  removes each waiter from its typed completion queue before it is readied. Both
  EL0 init processes prove a two-tick timeout against a live sibling, verify an
  unchanged sentinel output, then terminate and wait successfully on that same
  handle. The EL0 workload also creates the dedicated FAT child, verifies its
  `0x43` exit status through its process handle, closes the handle, and then
  continues normally. Controlled EL0 fault status handling remains implemented
  separately.

## Syscall And User-Memory Guarantees

- `NtWriteFile` copies from EL0 mappings only after validating the mapped user
  range; it does not dereference an untrusted user virtual pointer directly.
- `NtAllocateVirtual` and `NtQuerySystemInfo` copy their outputs only to fully
  validated writable EL0 mappings. `NtAllocateVirtual` owns mappings in the
  calling process instead of exposing a kernel virtual address.
- Automatic user-memory placement reuses released ranges, and nonzero requested
  bases are supported in the protected dynamic user-memory window.

## Fixed Live VirtIO/FAT Child Image

- QEMU attaches the boot image as an explicit modern `virtio-blk-device`. The
  kernel discovers the validated VirtIO block identity across the standard MMIO
  slots, negotiates only `VIRTIO_F_VERSION_1`, and uses a synchronous
  physical-page split queue to perform bounded read-only sector I/O.
- The live block source exposes at most 128 MiB. A sector-backed FAT32 reader
  validates volume geometry and reads root-level 8.3 `CHILD.ELF` on demand
  without retaining the disk image in UEFI or kernel heap memory.
- The I/O manager owns a private `ReadOnlyImage` descriptor for the fixed
  root-level `CHILD.ELF` name and 2 MiB cap, plus the sole public kernel-side
  `ReadOnlyFile` object that represents it. Neither can be created, named, or
  opened by EL0.
- The file object issues a synchronous kernel-only `IRP_MJ_READ` to its fixed
  I/O driver/device dispatch path. The handler performs the bounded FAT read,
  returns an explicit NTSTATUS and byte count, and the caller accepts it only
  after successful matching completion.
- `NtCreateProcess` permits only selector zero for the cached boot image or
  selector one for this I/O-owned live file; both sources use the same ELF and
  address-space validation path.
- `CHILD.ELF` is a dedicated static user executable: it writes a unique marker
  and calls `NtTerminateProcess(-1, 0x43)`. Both init processes launch it with
  selector one and validate the copied-out `0x43` completion status.

## Smoke-Test Evidence

`make smoke` is the current regression check. It rebuilds the UEFI loader,
kernel, and user ELFs, then boots QEMU headlessly. It requires proof of MMU and
high-half execution, virtual-memory probes, fault decoding, init-ELF mapping,
an EL0 timer preemption, successful EL0 context resume, validated copy-out,
fixed virtual-memory reuse, EL0 thread-handle waits in both init processes,
modern live VirtIO boot-disk initialization, two bounded live `CHILD.ELF`
file-object dispatches, matching I/O-manager IRP completions and mappings, two
child markers, parent-validated `0x43` completion status, process exits,
deferred thread reaps, typed process/thread wait validation, external typed
thread termination of queued, completed, and typed-completion-blocked targets
across typed thread and process completions with both infinite and finite waits
from both init processes,
current-target rejection in the scheduler, a `Ps: typed handle access rights
validated` marker proving wait and terminate denials on deliberately restricted
kernel handles, and continued System, Thread-A, and Thread-B activity.
It also requires two `[user-init] finite typed wait timeout validated` markers
alongside two `[user-init] process-wide blocked wait termination validated`
markers and two `Ps: current-process termination cleared 1 typed wait
registration(s)` markers. It also requires two
`[user-init] external process blocked wait termination validated` markers and
two external process-termination cancellation markers before the existing
sibling-termination and process-wait flows continue.

This validates the QEMU `virt`/TCG path. It is not hardware certification or
evidence of Windows application compatibility.
