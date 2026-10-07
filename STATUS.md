# CantayaOS Status

**Project:** CantayaOS by CantayaTech
**Target:** AArch64, UEFI, QEMU `virt` with OVMF
**Last verified:** 2026-10-07

## Current Milestone

**Objective:** Maintain the verified private VirtIO data-disk protocol for
durable root-file creation without changing the read-only boot ESP. The
protocol remains internal to private probes; do not expose a write syscall or
shell command until a separate public interface and contract are selected.
Keep the current three-boot smoke contract.

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
  workload. After both exit, the System thread redraws the framebuffer and
  starts a controlled `init.elf` copy in EL0 shell mode with its desktop. That process owns
  console input, parses the existing built-in commands, and writes the terminal
  through `NtWriteFile(-1)`; the EL1 prompt is retained only as a fallback if
  the process exits or cannot start. The terminal accepts PL011 serial and
  QEMU VirtIO-MMIO keyboard input. The periodic heartbeat and Thread-A/B
  demonstration loops are disabled.
  QEMU uses 1024x768 GOP mode when available. Keyboard decoding currently
  covers US ASCII, Shift, Caps Lock, Backspace, Enter, Ctrl-U, and Ctrl-L.
- The EL0 shell now starts a desktop on supported RGB/BGR displays. Its private
  buffers compose a background, taskbar, mouse cursor, draggable terminal and
  read-only Files windows. All window types resize and minimize; Terminal
  reflows visible text, Files adjusts its viewport, and Paint preserves hidden
  strokes across shrink/grow and retains its process when minimized. QEMU supplies
  a VirtIO absolute tablet alongside the keyboard. Bounded graphics/input/output syscalls share console ownership;
  closing input or falling back to EL1 resets the graphical session. The
  ordinary smoke boot validates interactions, actual framebuffer pixels, and
  recovery after a forced read timeout. The block reader retains pending DMA
  buffers until completion rather than reusing a timed-out request.
  Independent apps now own up to four bounded copied surfaces, receive routed
  focus/key/pointer input, and block on scheduler-managed window waits. Paint
  runs as a separate ELF and launches from the taskbar, Files or Terminal.
  Graphical terminal launches run asynchronously and release completion handles
  through zero-tick polling. Window close, process exit and desktop reset clean
  up surfaces and wake or cancel blocked waits. All FP/SIMD state is preserved
  across exceptions; context switches retain callee-saved vectors and new EL0
  processes start with cleared FP state. Regression probes hold distinct patterns
  in all 32 vector registers across syscalls and timer ticks.
  See [docs/desktop.md](docs/desktop.md) for controls, ABI and limits.
- `make smoke` now uses a private QMP monitor socket to send `help` through the
  VirtIO keyboard after terminal startup and requires its command response in
  the serial log. It also clears unfinished text with Ctrl-U, corrects a later
  character with Backspace, and requires the corrected `echo` response with
  no unknown-command output, in addition to all lifecycle counts.
- The same smoke run sends `echo serialprobe` through PL011 standard input and
  requires its distinct response in the captured serial log. The keyboard
  assertions and lifecycle counts remain required.
- A later keyboard command produces the checked mixed-case `AbCd` response:
  Shift uppercases `A`, Caps Lock uppercases `C`, and a second Caps Lock press
  restores lowercase `d`. Smoke retains the serial and lifecycle assertions.
- A subsequent Ctrl-L probe checks the UART clear sequence, a redrawn
  unfinished `echo saved` line, and its successful response after Enter.
- The built-in `clear` command also emits the clear sequence and restores a
  prompt; a later `echo clearok` succeeds through the VirtIO keyboard.
- Smoke sends `info`, `uptime`, and `mem` through PL011 in response order. It
  checks the version text and the numeric uptime and free-memory formats,
  each followed by a prompt.
- A PL011 `echo crlfprobe` terminated by CRLF returns one checked response and
  one prompt; smoke rejects an extra prompt from a second blank command.
- A deliberate PL011 `boguscmd` returns exactly one unknown-command error;
  `echo recovered` succeeds at the next prompt. Smoke rejects any additional
  unknown-command response.
- A paced PL011 probe fills the 128-byte command line, sends eight excess
  bytes, and requires eight bells. Only the accepted `echo` payload executes;
  `echo boundok` succeeds at the next prompt. All previous checks remain.
- Current EL0 coverage includes validated user-memory copy-in/copy-out, virtual
  allocation and free, thread create/wait/close/terminate, process
  create/wait/close/terminate, and system-information query.
- `NtWriteFile` accepts only the fixed `-1` console-output pseudo-handle and
  1–1,024 validated EL0 bytes. It rejects invalid handles, lengths, mappings,
  and unsupported bytes before output; accepted ASCII text reaches UART and
  framebuffer. Both init processes verify rejection and successful output in
  QEMU smoke. The pseudo-handle cannot be closed.
- `NtReadFile(-2)` accepts the fixed console-input pseudo-handle, a
  1–128-byte writable buffer, and a writable `u64` result count. It validates
  both full ranges before claiming input, returns `STATUS_TIMEOUT` without
  changing either output when no byte is ready, and excludes other EL0
  processes and the kernel shell while the owner is live. `NtClose(-2)` releases
  only the caller's claim; process exit lets the shell reclaim it.
- `NtWaitForConsoleInput(-2)` claims the same exclusive input pseudo-handle and
  blocks the caller until PL011 or keyboard input is ready. The System thread
  no longer polls readiness: PL011 RX and VirtIO keyboard IRQs wake one waiter
  of the live owner. Both devices drain into bounded software input FIFOs.
  Process-completion locking masks local IRQs so the wake path cannot
  interrupt a lock holder on the single core. Closing the claim invalidates its
  generation and wakes blocked callers with `STATUS_INVALID_HANDLE`.
  Thread/process termination removes console wait registrations before reaping.
  The EL0 shell waits after an empty bounded read instead of spinning.
- The private smoke boot blocks its input probe on both serial and keyboard
  readiness, then externally terminates a child blocked in the console wait.
  It requires the cancellation log, reclaims input without a stale waiter,
  and retains the full terminal and lifecycle contract plus ordinary boot.
- A separate private fallback boot lets the EL0 shell claim console input and
  exit deliberately. The EL1 prompt then accepts a VirtIO keyboard `help` and
  a PL011 `echo fallbackserial`; the unmodified boot retains EL0 ownership.
- EL0 opens bounded read-only FAT files through process-local typed handles.
  `NtReadFile` advances a per-handle offset and writes a zero count at EOF.
  Root and one-level 8.3 directory queries power shell `ls`; `cat` reads text
  files through the same handle path. The private probe checks invalid
  pointers and names, EOF, directory end, and stale-handle rejection after
  slot reuse.
- `NtCreateProcess` selector two loads a named root or one-level 8.3 ELF via
  the I/O-owned FAT reader and the existing ELF validation path. An optional
  printable argument string of at most 64 bytes is copied into child-owned
  memory and passed in initial EL0 `x1`. The shell waits for the process and
  reports its exit status. Smoke checks root and subdirectory `HELLO.ELF`, its
  `world` argument, missing and invalid images, and subdirectory text reads.
- The private smoke boot injects two invalid FAT geometry fields and an
  unreadable root sector through a faulted view of the live block device.
  The bounded reader rejects each case without a kernel panic; broader
  malformed-chain and ELF fixtures now live on its private disk copy.
- The private disk copy adds a cyclic `BROKEN.TXT` FAT chain, an ELF with a bad
  magic byte, and an ELF with a writable executable segment. EL0 verifies that
  failed file and process creation leave their output handles unchanged, then
  continues to open and run valid files. The FAT reader rejects repeated file
  clusters, bounds directory traversal, and publishes a file buffer only after
  the full chain succeeds. Smoke requires exactly two intended `HELLO.ELF`
  executions and all three boot paths.
- EL0 `help`/`?`, `info`, `uptime`, `mem`, `echo`, and `clear` preserve the
  earlier terminal responses and editing behavior. A fixed system-information
  class exposes elapsed 100 Hz ticks; an input-owner-only clear service redraws
  UART and framebuffer without admitting raw escape sequences through
  `NtWriteFile`.
- `NtTerminateThread` accepts only a parent-owned typed thread handle. It
  rejects the current thread, removes a non-current queued target safely,
  signals its handle completion with the requested status, and treats an
  already-complete target as a successful no-op.
- The smoke workload separately terminates siblings blocked on typed thread or
  process completions with either an infinite wait or a finite timeout. It
  reaps each raw record on a later context switch, lets every finite case pass
  its former deadline, then completes the original thread or child process.
  Cancelled registrations cannot wake stale state, and retained typed handles
  report requested status.
- Each init process runs two fixed mixed-lifecycle rounds. In each round, a
  thread and child process remain live while siblings hold 100-tick finite
  waits on their respective typed completions. External termination removes
  both waiters; a bounded 120-tick wait schedules deferred reaping and passes
  their former deadlines before the original targets complete. Each round
  closes its handles and frees its stacks before the next begins. Smoke
  requires four aggregate success markers and the corresponding typed,
  process, and timeout cancellation counts.
- Typed handle-table entries enforce distinct `WAIT` and `TERMINATE` rights
  before revealing their retained process or thread object. Current process
  and thread handles receive both lifecycle rights; denied rights return
  `STATUS_ACCESS_DENIED` without escaping the table.
- Typed handle values now include a process-local slot and issuance generation.
  Closing a handle advances that slot's generation. Boot-time checks cover
  same-type and both cross-type reuse directions and retain access-mask
  denial; in the second mixed-lifecycle round, both init processes verify that
  stale process and thread values cannot wait, terminate, or close their live
  replacements. Exhausted generations are not reissued.
- A private-table boot probe issues generation `u32::MAX - 1`, closes it, and
  confirms the exhausted slot is skipped on two subsequent
  insertions. Old lookup and close fail; a new slot remains usable with its
  own access rights. Smoke requires the exhaustion marker.
- Two private nonempty handle tables issue the same numeric value for a thread
  and a process with different rights. Lookups, close, and reuse remain
  table-local; smoke requires the numeric-collision boot marker.
- A private table with one live `WAIT`-only handle rejects null, the current-
  process pseudo-handle, zero and out-of-range slots, and non-issued or
  exhausted generations. Each invalid lookup returns `Invalid`, each close
  fails, and the live handle retains its rights. Smoke requires the malformed-
  value boot marker.
- In the second round, each init process additionally closes the completed
  process target and reuses its slot for a live thread, then closes the
  completed thread target and reuses its slot for a live process. Both stale
  cross-type values fail wait, terminate, and close with
  `STATUS_INVALID_HANDLE` while their replacements remain live. The new
  thread and process complete with checked statuses, and their handles and
  extra thread stack are released. Smoke requires two cross-type EL0 markers
  without changing the four mixed-lifecycle markers or cancellation counts.
- The second round also reissues the freed process slot to one more live
  process and then one more live thread. While each replacement is live, all
  retained older values fail wait and close with `STATUS_INVALID_HANDLE`.
  Both newest handles complete with checked statuses and are closed. Smoke
  requires two multi-generation markers, with the mixed-lifecycle markers
  and cancellation counts unchanged.
- Both init processes give fresh selector-zero children with empty handle
  tables a live parent process handle value and, separately, a live parent
  thread handle value. Each child rejects wait, both typed termination calls,
  and close with `STATUS_INVALID_HANDLE` and exits with a checked status. The
  parent then completes each original target through its own handle. Smoke
  requires four child and two parent isolation markers while retaining the
  two-round lifecycle counts. This does not assert that equal numeric values
  in different nonempty tables refer to the same object.
- Both init processes reject null, unmapped, read-only, and cross-page output
  pointers for thread and process creation. A cross-page sentinel stays
  unchanged; the next valid thread and process handles use the exact next
  generations, complete with checked statuses, and release their resources.
  Smoke requires two output-failure markers and absence of a failure-only
  target marker. Post-insertion copy-out rollback is source-reviewed only.
- Both init processes reject a mapped non-executable thread entry and
  misaligned or unmapped stack tops with a valid output pointer. Its sentinel
  stays unchanged; the next valid thread receives the exact next handle
  generation, completes with a checked status, and releases its stack. Smoke
  requires two entry-and-stack preflight markers.
- Both init processes reject invalid timeout pointers and out-of-range
  timeout values, and poll live handles with a zero-tick timeout, and read-only or unmapped completion outputs for live thread
  and process handles. Bad timeout arguments preserve their passed output
  sentinel; bad output pointers return `STATUS_ACCESS_VIOLATION`. A valid
  two-tick wait times out without writing its output; termination followed by an
  infinite wait returns each checked status. Smoke requires two typed-wait
  argument preflight markers. The no-registration property for invalid
  arguments follows the source validation order, rather than a direct EL0
  registration-count assertion.
- Both init processes also reject an eight-byte timeout input and a four-byte
  completion output that start in a mapped scratch page and cross into its
  explicitly unmapped neighbor. The passed output and mapped-side sentinels
  stay unchanged. Each live target subsequently times out on a valid finite
  wait and completes with its checked status; the scratch page and thread
  stack are released. Smoke requires two cross-page wait markers.
- Each init process also starts two controlled fresh processes whose first
  thread handles both equal numeric `1`. The child completes and closes its
  own thread; the parent still sees a timeout on its live thread and then
  completes it with a distinct status. Smoke requires two child, two parent,
  and two init-level collision markers, preserving the two-round counts.
- `NtWaitForSingleObject` accepts a validated relative timeout of one to 1,000
  scheduler ticks through its existing timeout pointer. Expiration returns
  `STATUS_TIMEOUT`, never writes a completion status, and removes the waiter
  from the typed completion object before it can be signaled later.
- The scheduler tracks typed process and thread completion waiters separately
  from ready threads. Wake, timeout, thread termination, and process
  termination remove that tracking before a raw thread record can be reaped.
- `NtCreateProcess` can load the retained boot-validated `init.elf`, the fixed
  kernel-owned live VirtIO/FAT `CHILD.ELF`, or a bounded named FAT image. All
  three use the same ELF and address-space validation path.

Detailed guarantees and validation evidence live in
[docs/verified-features.md](docs/verified-features.md). Stable subsystem design
lives in [docs/architecture.md](docs/architecture.md).

## Latest Verified Milestone

The EL0 desktop displays a terminal and read-only file browser, handles
structured keyboard and mouse input, and supports window focus, dragging,
resizing, minimizing, closing and taskbar reopening. Independent EL0 applications
now use bounded copied surfaces and routed input. Paint supports drawing, color selection and
clearing; the graphical terminal runs programs asynchronously. `make smoke`
retains all three boot paths and verifies window ownership, memory rejection,
queue overflow, wait cancellation, SIMD preservation, two simultaneous apps,
terminal responsiveness, lifecycle cleanup and actual framebuffer contents.
Resize probes check preserved rows, zeroed growth, rejected dimensions, revision
races and blocked app wakeup. Interaction tests cover terminal command reflow,
Files selection/preview resizing and Paint retention through minimize/restore.
The separate `CANTDATA` VirtIO disk now has a bounded one-cluster root-file
create protocol with mirrored transaction records, explicit flushes, recovery,
and data checksums. Private QEMU probes prove reboot persistence, write and
flush failure recovery, eight durable interruption checkpoints, capacity and
root-directory exhaustion, and FAT/directory/journal/payload corruption
rejection. The boot ESP and all public file interfaces remain read-only.

## Latest Planning Decision

The [lifecycle contract review](docs/lifecycle-contract-review.md) identified
the closed-slot aliasing gap and selected issuance generations. Output-pointer
prevalidation now has EL0 evidence for both creation calls; the later
copy-out rollback branches remain source-reviewed. Entry and stack rejection,
ordinary typed-wait preflight, and cross-page wait arguments have EL0 evidence.
Terminal commands, editing, PL011 CRLF suppression, unknown-command recovery,
and the fixed input-line capacity are checked through smoke input. GitHub
Actions builds the project and runs the same headless QEMU smoke test on
Ubuntu with AArch64 UEFI firmware. The tested nightly is pinned to avoid a
newer toolchain's UEFI linker regression. Console output remains a fixed
pseudo-handle operation; input uses `-2` while typed read-only file handles
share `NtReadFile`. The EL1 parser remains a
fallback; smoke rejects unexpected fallback during the EL0 command checks.
The smoke harness uses explicit QMP key down/up events and a private serial
socket, with bounded pacing for the PL011 line-capacity probe. A QEMU stall
diagnostic exposed a single-core IRQ re-entry into the process-completion
mutex; completion access now masks local IRQs.
The storage review selected a separate data disk for internal transactional
creation. [docs/storage-integrity.md](docs/storage-integrity.md) records the
verified durability and recovery gate; public write exposure remains deferred.

## Current Milestone Detail

The separate data disk is identified by its FAT32 volume label and mounts
independently of the boot ESP. Its bounded create protocol and all required
private QEMU recovery tests are complete. Do not expose a write syscall until
a future public API preserves the completed integrity contract.

### Follow-On Candidates

- Keep the two-round mixed-lifecycle workload as a fixed regression case for
  future scheduler or lifecycle changes.

## Hard Constraints And Do Not Implement Yet

- The low device identity mapping remains only for bootstrap and kernel-mode
  execution; it is not copied into any user TTBR0 root. Each user root retains
  only the supervisor-only `0x4000_0000` kernel-image alias required until the
  kernel is linked fully in the high half. EL1 support paths otherwise use
  high-half mappings.
- `NtCreateProcess` accepts selector zero for the kernel-owned boot image,
  selector one for the fixed I/O-owned `CHILD.ELF`, and selector two for a
  bounded root or one-level 8.3 FAT path. Selector two copies at most 64
  printable argument bytes to child-owned memory and passes their pointer in
  initial EL0 `x1`. Do not add raw user ELF input, inherited handles, a general
  command-line parser, or security tokens. It returns a parent-owned handle.
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
  `NtTerminateProcess(process_handle, status)` applies the same removal and
  cancellation sequence to all ready and typed-completion-blocked threads in
  its non-current target process. Do not add process groups, job objects, or
  cross-process thread creation. `NtTerminateThread(thread_handle, status)`
  accepts only a parent-owned typed thread handle; it rejects a target that is
  `CURRENT`, removes a ready target or tracked typed-completion waiter, signals
  the requested final status, and defers raw-stack reclamation. A repeated
  request after completion succeeds without replacing the status.
- `NtAllocateVirtual` accepts only the current-process pseudo-handle. It has
  automatic first-fit reuse and page-aligned fixed placement in the dynamic
  user-memory window, but no reservation/commit split or protection changes.
- The fixed `CHILD.ELF` image keeps its synchronous kernel-owned IRP path.
  Additional read-only FAT opens use process-local typed file handles and
  bounded per-handle offsets. Paths are limited to 8.3 names in the root or
  one subdirectory. Do not add writes, arbitrary devices or user-chosen
  offsets, pending I/O, or I/O cancellation yet.
- Most NT syscall entries, I/O facilities, object access checks, and the
  security model remain scaffolding rather than finished operating-system
  services.
- Future lifecycle work must retain the verified cancellation of blocked typed
  thread and process waits and the fixed two-round queue-cleanliness proof.
- Security tokens/access checks, SMP, per-CPU scheduling, and GICv3 are
  deferred after lifecycle controls.

## Current Blockers

None recorded for local or CI smoke.

## Verification Requirements

Run `make smoke` for meaningful kernel, MMU, scheduler, syscall, process, or
I/O changes, including keyboard and pointer input. It exercises a private smoke-flagged
disk, an unmodified normal boot, and a private fallback boot; it is the
regression check for the QEMU
`virt`/TCG path; a successful compile alone is not completion. Smoke injects
`help`, Ctrl-U,
Backspace, Shift, Caps Lock, and Ctrl-L key events and checks responses; additional
keyboard behavior changes need targeted key-event checks. This is not hardware
certification or evidence of Windows application compatibility.

## Detailed References

- [Documentation map](docs/README.md)
- [Stable architecture](docs/architecture.md)
- [Verified features and smoke-test evidence](docs/verified-features.md)
