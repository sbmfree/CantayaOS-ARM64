# CantayaOS Status

**Project:** CantayaOS by CantayaTech
**Target:** AArch64, UEFI, QEMU `virt` with OVMF
**Last verified:** 2026-09-27

## Current Milestone

**Objective:** Verify PL011 CRLF input runs one command with one following
prompt in the existing QEMU smoke run.

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
  workload. After both exit, the System thread clears the framebuffer boot log
  and starts a CantayaOS banner and lower terminal pane. The terminal accepts
  built-in commands from either PL011 serial or QEMU's VirtIO-MMIO keyboard;
  the periodic heartbeat and Thread-A/B demonstration loops are disabled.
  QEMU uses 1024x768 GOP mode when available. Keyboard decoding currently
  covers US ASCII, Shift, Caps Lock, Backspace, Enter, Ctrl-U, and Ctrl-L.
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
- Current EL0 coverage includes validated user-memory copy-in/copy-out, virtual
  allocation and free, thread create/wait/close/terminate, process
  create/wait/close/terminate, and system-information query.
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
- Both init processes reject invalid timeout pointers, zero and out-of-range
  timeout values, and read-only or unmapped completion outputs for live thread
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
- `NtCreateProcess` can load only the retained boot-validated `init.elf` or the
  fixed, kernel-owned live VirtIO/FAT `CHILD.ELF` source. Both follow the same
  ELF and address-space validation path.

Detailed guarantees and validation evidence live in
[docs/verified-features.md](docs/verified-features.md). Stable subsystem design
lives in [docs/architecture.md](docs/architecture.md).

## Latest Verified Milestone

The headless QEMU smoke test now verifies `info`, `uptime`, and `mem` through
PL011, including their distinct response formats and prompts. Keyboard input
and every lifecycle count remain required.

## Latest Planning Decision

The [lifecycle contract review](docs/lifecycle-contract-review.md) identified
the closed-slot aliasing gap and selected issuance generations. Output-pointer
prevalidation now has EL0 evidence for both creation calls; the later
copy-out rollback branches remain source-reviewed. Entry and stack rejection,
ordinary typed-wait preflight, and cross-page wait arguments have EL0 evidence.
Terminal commands and editing are checked through keyboard and PL011 smoke
input. PL011 CRLF suppression is implemented but not yet tested as an input
pair; a stray LF must not run a second blank command.

## Recommended Next Milestone

Send a unique PL011 `echo` command terminated by CRLF, require exactly one
command response and no extra blank-command prompt, and retain all keyboard,
status-command, and lifecycle checks. Keep this test-only.

### Follow-On Candidates

- Keep the two-round mixed-lifecycle workload as a fixed regression case for
  future scheduler or lifecycle changes.

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
- The only completed I/O request is a synchronous kernel-owned read by the
  fixed `CHILD.ELF` file object. Do not add EL0 filenames, arbitrary devices or
  offsets, pending I/O, or cancellation.
- Most NT syscall entries, I/O facilities, object access checks, and the
  security model remain scaffolding rather than finished operating-system
  services.
- Future lifecycle work must retain the verified cancellation of blocked typed
  thread and process waits and the fixed two-round queue-cleanliness proof.
- Security tokens/access checks, SMP, per-CPU scheduling, and GICv3 are
  deferred after lifecycle controls.

## Current Blockers

None recorded for bounded PL011 CRLF smoke automation.

## Verification Requirements

Run `make smoke` for meaningful kernel, MMU, scheduler, syscall, process, or
I/O changes, including keyboard input. It is the regression check for the QEMU
`virt`/TCG path and must demonstrate the required runtime markers; a
successful compile alone is not completion. Smoke now injects `help`, Ctrl-U,
Backspace, Shift, Caps Lock, and Ctrl-L key events and checks responses; additional
keyboard behavior changes need targeted key-event checks. This is not hardware
certification or evidence of Windows application compatibility.

## Detailed References

- [Documentation map](docs/README.md)
- [Stable architecture](docs/architecture.md)
- [Verified features and smoke-test evidence](docs/verified-features.md)
