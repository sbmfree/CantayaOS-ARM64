# Verified Features

This is the detailed verified baseline for CantayaOS. It records completed
behavior and technical guarantees, not the current roadmap. For what to change
next, read [STATUS.md](../STATUS.md).

## Boot, Hardware, And Kernel Foundation

- The System kernel thread starts a bounded, line-oriented terminal after both
  boot validation processes exit. It accepts printable ASCII, Enter,
  Backspace, Ctrl-U, and Ctrl-L from PL011 serial or QEMU's VirtIO-MMIO
  keyboard, then handles `help`, `info`, `uptime`, `mem`, `echo`, and `clear`
  without a user-mode file or process interface. The keyboard driver drains
  its event queue on IRQs, decodes a US ASCII key map with Shift and Caps Lock,
  and leaves LED feedback unimplemented. `make smoke` now injects `help` through a
  private QMP monitor and requires its terminal response. The same smoke run
  verifies Ctrl-U clears unfinished text and Backspace corrects an `echo`
  command before submission. The smoke run also sends `echo serialprobe`
  through PL011 and requires its distinct response. A later keyboard command
  checks Shift and Caps Lock with the mixed-case `AbCd` response, including
  lowercase after Caps Lock is reset. A separate earlier QEMU injection ran
  `echo window` successfully. Smoke also checks Ctrl-L's clear sequence and
  restored unfinished command. The `clear` command emits the clear sequence
  and leaves a prompt that runs `echo clearok`. Smoke also runs `info`,
  `uptime`, and `mem` via PL011 and checks their version and numeric response
  formats. A CRLF-terminated PL011 `echo` produces one response and one
  prompt. One deliberate unknown command reports its error, and a later
  `echo recovered` succeeds. A paced PL011 probe fills all 128 input slots,
  checks eight bells for excess bytes, runs only the accepted `echo` payload,
  and confirms the next command still works. The periodic System heartbeat
  and Thread-A/B liveness loops are disabled. After the programs exit, the
  framebuffer clears boot logs, draws a CantayaOS version banner, and mirrors
  terminal text in a lower pane that scrolls without moving the banner. QEMU's
  OVMF GOP uses 1024x768 when available; a QEMU screendump confirmed the pane.
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
- Both init processes reject null, unmapped, read-only, and cross-page output
  pointers for `NtCreateThread` and `NtCreateProcess` before publishing a
  handle. A sentinel at the writable edge of each cross-page range remains
  unchanged. The next valid thread and process creations receive the exact
  next generations of the previously closed slot, complete with checked
  statuses, and release their resources. A failure-only entry is watched by
  smoke and must never run. The post-insertion copy-out rollback branches are
  source-reviewed but not deterministically reached by this EL0 probe.
- Both init processes reject a mapped non-executable thread entry and
  misaligned or unmapped stack tops before publishing a handle. The writable
  output sentinel stays unchanged after all three failures. A subsequent
  valid thread receives the exactly predicted next generation, completes
  with a checked status, and releases its stack. `make smoke` requires two
  entry-and-stack preflight markers and rejects the failure-only entry marker.
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
- Each init process also runs two bounded mixed-lifecycle rounds. In each
  round, a live thread and child process coexist with sibling threads holding
  100-tick finite waits on their typed completions. The parent externally
  terminates both waiters, verifies statuses `0x63` and `0x64`, and enters a
  120-tick finite wait on the still-live thread target. That scheduling
  interval reaps both raw waiter records and passes both former deadlines
  before the parent completes the original thread and process with statuses
  `0x65` and `0x66`. It closes the round's handles and frees its thread
  stacks before starting the next round. Four aggregate
  `[user-init] mixed finite lifecycle cancellation validated` markers prove
  both init processes completed both rounds.
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
- Each typed handle encodes a process-local slot and issuance generation.
  Closing it advances the slot generation before reuse; old values fail lookup
  before type or access checks and cannot close the replacement. A boot-time
  probe covers thread-to-thread, process-to-process, and both cross-type reuse
  directions, including preserved access-mask denial after reuse. A separate
  private-table boot probe seeds a closed slot just before `u32::MAX`, issues
  and closes the last usable generation, then confirms the exhausted slot is
  skipped in favor of a new slot. Old values cannot look up or close the new
  handle, whose access rights remain intact; a further issuance still skips
  the exhausted slot. In the second mixed-lifecycle round, both init processes
  verify that saved old process and thread handles return
  `STATUS_INVALID_HANDLE` from wait,
  terminate, and close, then complete their live replacements successfully.
  After the original round targets finish, each init process also closes the
  completed process handle and reuses its slot for a live thread, then closes
  the completed thread handle and reuses its slot for a live process. Each old
  cross-type value fails wait, terminate, and close while both replacements
  remain live; both replacements subsequently report their requested exit
  statuses and are closed. The freed process slot is then issued to one more
  live process and one more live thread in succession. At each step, every
  retained older value fails both wait and close with `STATUS_INVALID_HANDLE`
  while the newest object remains live. Each newest handle still reports its
  requested exit status, and the extra thread stack is freed after its reuse.
- A second private-table boot probe issues the same numeric slot/generation
  value in two nonempty tables, one naming a thread with `WAIT` and the other
  a process with `TERMINATE`. Lookups resolve each table's own object and
  rights. Closing and reusing the value in one table leaves the other table's
  object and rights unchanged.
- A third private-table boot probe keeps one live `WAIT`-only entry while
  checking null, the current-process pseudo-handle, zero and out-of-range
  slots, and non-issued or exhausted generations. Every lookup returns
  `Invalid` before access checks and every close fails; the live entry keeps
  its positive `WAIT` access and denied `TERMINATE` access throughout.
- Each init process then passes a live parent-owned process handle value to a
  fresh selector-zero child with an empty handle table, and repeats with a
  live parent-owned thread handle. Each child verifies that wait, both typed
  termination calls, and close return `STATUS_INVALID_HANDLE` for that value,
  then exits with a checked success status. The parent still terminates and
  waits on each original target through its own handle, closes the handles,
  and frees the thread stack. This proves the empty-child-table boundary; it
  does not assert that equal numeric values in different nonempty tables name
  the same object.
- Each init process also spawns a controlled fresh process whose first thread
  handle is numeric `1`. That process spawns a child whose own first thread
  handle is also `1`. The child terminates, waits for, and closes its thread;
  the parent then observes a finite timeout on its still-live thread before
  terminating and waiting for it with a distinct status. Both thread stacks
  are freed, and checked process statuses carry the result back to the init
  processes. This exercises table-local resolution in two nonempty EL0 tables.
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
- Both init processes probe live typed thread and process handles with an
  unmapped timeout pointer, zero and 1,001-tick values, and read-only or
  unmapped completion outputs. Rejected timeout arguments leave their passed
  writable output sentinel unchanged; unwritable outputs return
  `STATUS_ACCESS_VIOLATION`. A valid two-tick wait returns `STATUS_TIMEOUT`
  without writing its output; after termination, an infinite wait
  copies each requested completion status. Source ordering shows that the
  rejected arguments cannot register a waiter; the smoke test does not count
  registration state directly for these failures.
- Both init processes map a dedicated scratch page and explicitly unmap its
  neighbor, then probe an eight-byte timeout input starting four bytes before
  the boundary and a four-byte completion output starting two bytes before
  it. Each returns `STATUS_ACCESS_VIOLATION`; the passed status output and
  mapped-side sentinel remain unchanged. A subsequent valid finite wait
  times out while the target is live, and an infinite wait returns its checked
  termination status. Both typed handle kinds are closed, and the scratch
  page and thread stack are released. Source validation order prevents the
  rejected calls from registering a waiter; smoke checks their statuses and
  sentinels, not the registration count directly.

## Syscall And User-Memory Guarantees

- `NtWriteFile` accepts only the fixed, non-closable `-1` console-output
  pseudo-handle. It copies 1–1,024 bytes only after validating the full mapped
  EL0 range, accepts printable ASCII plus BEL, Backspace, CR, and LF, then
  mirrors the text to UART and framebuffer. It rejects invalid handles,
  lengths, mappings, and unsupported bytes before producing any output.
  Both init processes check these failures and a valid write in QEMU smoke;
  the framebuffer mirror is source-reviewed, not pixel-compared by smoke.
- `NtReadFile` accepts only the fixed `-2` console-input pseudo-handle with
  capacity 1–128 and a writable `u64` count pointer. It validates both full
  writable EL0 ranges before claiming input. With no data it returns
  `STATUS_TIMEOUT` and leaves both outputs unchanged; another process gets
  `STATUS_ACCESS_DENIED`. A successful call copies immediately available
  PL011 or VirtIO-keyboard bytes and the count. `NtClose(-2)` releases only the
  owner's claim; a completed owner is reclaimed by the next shell read.
- `NtWaitForConsoleInput(-2)` registers a cancellable scheduler wait on the
  exclusive input claim. PL011 RX and VirtIO input IRQs wake one waiter
  without consuming data. The System thread does not probe readiness in its
  hot loops. `NtClose(-2)` invalidates the claim generation; external
  termination removes the registration before
  the raw thread can be reaped. The EL0 shell sleeps after an empty read.
  The smoke probe blocks on serial and keyboard readiness and externally
  terminates a child blocked in this wait, then reclaims input successfully.
- The controlled EL0 shell uses fixed class 0 (free pages) and class 1
  (elapsed 100 Hz ticks) of `NtQuerySystemInfo` for `mem` and `uptime`.
  `NtClearConsole` redraws UART and framebuffer only for the live input owner;
  a competing process receives `STATUS_ACCESS_DENIED`. Raw escape bytes remain
  disallowed in `NtWriteFile`.
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
from both init processes, two bounded mixed finite thread/process cancellation
rounds in each init process, current-target rejection in the scheduler, and a
`Ps: typed handle access rights validated` marker proving wait and terminate
denials on deliberately restricted
kernel handles, VirtIO keyboard initialization, and the terminal prompt after
both init processes complete.
It also requires two `[user-init] finite typed wait timeout validated` markers
alongside two `[user-init] process-wide blocked wait termination validated`
markers and two `Ps: current-process termination cleared 1 typed wait
registration(s)` markers. It also requires two
`[user-init] external process blocked wait termination validated` markers and
two external process-termination cancellation markers before the existing
sibling-termination and process-wait flows continue. The mixed phase adds four
`[user-init] mixed finite lifecycle cancellation validated` markers and raises
the minimum external thread-termination evidence to sixteen typed-registration
cancellations, eight process-wait cancellations, and twelve finite-timeout
cancellations. It also requires a boot-time
`Ps: stale typed handle reuse rejected` marker and two
`[user-init] stale typed handles rejected after reuse` markers. The cross-type
EL0 extension also requires two
`[user-init] cross-type stale handles rejected after reuse` markers. The
multi-generation EL0 extension also requires two
`[user-init] multi-generation stale handles rejected after churn` markers.
The process-local isolation phase requires four
`[user-init] isolated child rejected parent handle` markers and two
`[user-init] process-local parent handles validated` markers. These checks
and the boot-time `Ps: exhausted typed handle slot skipped` marker passed
`make smoke`, along with the boot-time numeric-collision marker. The EL0
collision phase additionally requires two each of
`[user-init] colliding child thread handle completed`,
`[user-init] colliding parent thread handle remained live`, and
`[user-init] EL0 numeric handle collision validated`. The earlier
two-round milestone passed two consecutive smoke runs. The final private-table
negative probe adds the boot-time `Ps: malformed typed handle values rejected`
marker. Output-pointer validation adds two
`[user-init] create output failures left no handles` markers and rejects the
failure-only target marker; `make smoke` passed with these and all prior checks.
Thread entry and stack preflight adds two
`[user-init] thread entry and stack preflight validated` markers; `make smoke`
passed with these and all prior checks.
Typed-wait argument preflight adds two
`[user-init] typed wait argument preflight validated` markers; `make smoke`
passed with these and all prior checks.
Cross-page wait preflight adds two
`[user-init] cross-page typed wait preflight validated` markers; `make smoke`
passed with these, the previous lifecycle checks, and the keyboard and
terminal startup markers.
Console-output preflight adds two
`[user-init] EL0 console output contract validated` markers. Each init
process checks invalid-handle, close, zero and excessive lengths, unsupported
bytes, an unmapped pointer, and a valid write. `make smoke` passed with all
previous terminal and lifecycle assertions.
Console-input preflight uses a controlled extra init copy on a private
smoke-flagged disk. It checks invalid handle and close, zero and excessive
capacity, unmapped and cross-page output ranges, untouched outputs on an empty
read, denial of a competing process, PL011 `@` and keyboard `k` reception,
close and double-close, then externally terminates a child blocked in the new
console wait before reclaiming input and exiting with a fresh claim. The probe
uses the wait syscall for both `@` and `k`, and the competing process verifies
that it cannot wait or clear the terminal. The System thread then starts
the EL0 command loop; smoke waits for its readiness marker before injecting
keyboard `help`. `make smoke` also boots the unmodified disk separately and
verifies that the ordinary EL0 terminal accepts keyboard `help` and serial
`echo normalboot` without running the probe. Both boots passed locally with
the prior lifecycle and terminal assertions retained.

The EL0 command loop preserves the 128-byte line limit, editing controls,
CRLF suppression, and the existing `help`, `info`, `uptime`, `mem`, `echo`, and
`clear` responses. Smoke runs the full serial/keyboard command sequence after
the EL0 readiness marker and rejects an unexpected EL1 fallback. It also
checks the `?` alias and a bare `echo` response. The fallback path itself is
source-reviewed, not yet exercised by a dedicated QEMU fault-injection case.

The terminal and keyboard change also passed `make smoke`; the smoke test now
injects `help` and an edited `echo` command through the keyboard and requires
both responses, with no unknown-command output. A separate earlier injection
verified `echo window`. A QEMU framebuffer screendump confirmed the 1024x768
mode and banner with the lower terminal pane. The same smoke run additionally
sends a PL011 serial `echo` command and checks its distinct response. It also
requires the mixed-case `AbCd` response from Shift/Caps Lock keyboard events.
Ctrl-L emits the UART clear sequence, redraws the unfinished `echo saved`
input, and still produces its `saved` response after Enter.
The built-in `clear` command emits the clear sequence and prompt, and the
subsequent `echo clearok` command returns its checked payload.
PL011 `info`, `uptime`, and `mem` commands return the checked version and
numeric uptime/free-memory formats, each followed by a prompt.
A CRLF-terminated PL011 `echo crlfprobe` returns one checked response and no
extra blank-command prompt.
The only unknown-command response is the deliberate `boguscmd` error; a
subsequent `echo recovered` succeeds at the next prompt.
A paced PL011 probe fills the 128-byte input line, checks eight consecutive
overflow bells, rejects the excess `boguscmd` suffix, and runs the accepted
`echo` payload. The next `echo boundok` succeeds. All earlier keyboard,
serial, and two-round lifecycle assertions remain required.

PL011 RX and VirtIO keyboard IRQs now wake the blocked console reader without
System-thread readiness polling or a timer fallback. A bounded PL011 software
FIFO and the decoded-key FIFO retain input until `NtReadFile` consumes it.
The private smoke boot requires both IRQ counters after its serial/keyboard
wait probe, then retains the full command, cancellation, and lifecycle
contract; the separate normal boot also passes. A QEMU hang during development
was traced to an IRQ checking process liveness while the interrupted code held
the process-completion mutex. Completion operations now mask local IRQs while
holding that mutex.

This validates the QEMU `virt`/TCG path. It is not hardware certification or
evidence of Windows application compatibility.
