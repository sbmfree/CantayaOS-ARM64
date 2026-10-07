# CantayaOS Documentation

Read the smallest document that answers the task at hand.

- [STATUS.md](../STATUS.md): current work, hard constraints, next milestone,
  blockers, and verification requirements. Start here for implementation work.
- [PLAN.md](../PLAN.md): ordered roadmap, milestone gates, and target data flow.
- [architecture.md](architecture.md): stable system design and subsystem
  boundaries, including the desktop, terminal, and VirtIO input devices. Read
  only the relevant subsystem section.
- [verified-features.md](verified-features.md): detailed completed behavior,
  ownership rules, restrictions, and smoke-test evidence, including QEMU
  keyboard input. Search for the relevant subsystem or guarantee.
- [desktop.md](desktop.md): desktop controls, graphics/input ABI, ownership,
  rendering limits, and QEMU interaction checks.
- [storage-integrity.md](storage-integrity.md): verified acceptance and recovery
  gates for the private writable data disk.
- [lifecycle-contract-review.md](lifecycle-contract-review.md): the handle
  reuse finding, decision, and verification outcome.
- [SECURITY.md](../SECURITY.md): current support and vulnerability reporting
  information.
- Git history: historical sequence and rationale not needed for ordinary
  implementation. Consult it only when the current documents and source do
  not answer a historical question.
