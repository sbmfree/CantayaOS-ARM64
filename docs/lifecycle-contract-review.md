# Lifecycle Contract Review

**Reviewed:** 2026-09-27
**Scope:** Typed process and thread handles, waits, closes, and termination after
the verified two-round mixed-lifecycle workload.

## Finding

At review time, the [handle table](../kernel/src/executive/ob/handle.rs) used
the slot index plus one as the entire handle value. `close` emptied that slot,
and the next `insert` returned the same value. A saved closed handle could
therefore access the replacement object. `lookup_with_access` checked the
replacement entry's rights and type without recognizing the earlier issuance.
This source-level finding motivated the generation-aware implementation.

The gap affects the existing `NtClose`, `NtWaitForSingleObject`,
`NtTerminateThread`, and `NtTerminateProcess` paths because they all resolve
the caller's numeric value through its process-owned handle table. A stale
handle may name a new object of either type; a stale `NtClose` can remove the
new entry. Retained completion objects and deferred thread reaping do not
prevent this aliasing because the error occurs at handle lookup.

## Decision

The selected implementation milestone was **closed-handle non-aliasing**. Each
issued table slot now has an opaque issuance generation. Lookup and close
require a matching slot and generation; closing advances the generation before
reuse. Exhausted generations are skipped, and no real handle equals zero or
the current-process pseudo-handle `u64::MAX`.

Keep the existing table-local `WAIT` and `TERMINATE` rights and typed object
checks. Handle values remain process-local. Existing syscall callers treated
handles as opaque; the EL0 regression inspects the slot portion only to prove
that its saved and replacement values refer to the same table slot.

## Verification

1. The boot-time System-thread probe checks same-type and both cross-type
   replacements. Old lookup and close fail; the replacement remains valid.
   It also checks that a reused slot retains its new entry's access rights.
2. In the second mixed-lifecycle round, both EL0 init processes confirm that
   saved process and thread handles name reused slots but differ from new
   values. Stale wait, terminate, and close calls return
   `STATUS_INVALID_HANDLE`. The new thread and process handles still observe
   their own target statuses after the old-value checks.
3. `make smoke` passed with the boot probe, two EL0 stale-handle markers, four
   mixed-lifecycle markers, and the existing cancellation counts.
4. Both EL0 init processes additionally reuse a completed process handle's
   slot for a live thread and a completed thread handle's slot for a live
   process. Old wait, terminate, and close calls return
   `STATUS_INVALID_HANDLE`; the replacement thread and process still complete
   with their requested statuses. `make smoke` passed with two cross-type EL0
   markers and the existing mixed-lifecycle cancellation counts.
5. Both init processes then reuse the first slot for another live process and
   thread. At each issuance, every older value fails wait and close while the
   newest handle remains usable through checked completion and close. `make
   smoke` passed with two multi-generation markers and unchanged two-round
   cancellation counts.
6. Each init process gives a fresh selector-zero child with an empty handle
   table the numeric value of a live parent process handle and, in a separate
   probe, a live parent thread handle. The children reject wait, both typed
   termination calls, and close with `STATUS_INVALID_HANDLE`; the parents
   afterward complete both original targets through their own handles. `make
   smoke` passed with four child and two parent isolation markers. This does
   not claim numeric handle values are globally unique.
7. A private-table System-thread probe seeds a closed slot at `u32::MAX - 1`,
   issues and closes that last usable generation, and verifies that the now
   exhausted slot is skipped on subsequent insertions. Old lookup and close
   fail; the replacement in another slot retains its access-mask behavior.
   `make smoke` passed with `Ps: exhausted typed handle slot skipped` and all
   prior lifecycle markers.
8. Two private nonempty tables issue the same numeric handle for different
   typed objects and access masks. Lookup, close, and reuse in one table do
   not alter the other. `make smoke` passed with the process-local numeric
   collision marker and prior checks.
9. Each init process starts a fresh controlled process that creates thread
   handle `1`, then a child whose own first thread handle is also `1`. The
   child completes and closes its thread; the parent's finite wait on its
   identically numbered handle still times out before it completes that
   thread with a distinct status. `make smoke` passed with two child, two
   parent, and two init-level EL0 collision markers.
10. A private table with a live `WAIT`-only handle rejects null, the pseudo-
    handle, zero and out-of-range slots, and non-issued or exhausted
    generations on lookup and close. The live handle and its rights remain
    unchanged after every rejection. `make smoke` passed with the malformed-
    value boot marker and all prior checks.
11. Both init processes reject invalid or unwritable output pointers for
    `NtCreateThread` and `NtCreateProcess`, retain a cross-page sentinel, and
    then receive the exact next handle generations on valid creations. A
    failure-only entry does not run. `make smoke` passed with two output-
    failure markers and prior checks. Post-insertion copy-out rollback is
    source-reviewed, not exercised by these prevalidation failures.
12. Both init processes reject a non-executable mapped thread entry and
    misaligned or unmapped stack tops before publishing a handle. The output
    sentinel remains unchanged, and the next valid creation receives the
    expected generation and completes with a checked status. `make smoke`
    passed with two entry-and-stack preflight markers and prior checks.

The [hard constraints](../STATUS.md#hard-constraints-and-do-not-implement-yet)
remain in force for later milestones.
