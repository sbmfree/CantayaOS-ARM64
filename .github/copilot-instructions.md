# Copilot Instructions

- Treat [STATUS.md](../STATUS.md) as the roadmap and source of current
  priorities.
- Read `STATUS.md` progressively: start with the current milestone, hard
  constraints, next actions, and verification requirements.
- Read [architecture.md](../docs/architecture.md) only for the subsystem
  relevant to the task.
- Search [verified-features.md](../docs/verified-features.md) only for the
  relevant subsystem or guarantee; never read it all by default.
- Start with the smallest relevant code scope and expand only when evidence
  requires it. Preserve hard constraints and do not redesign stable systems
  without need.
- If documents conflict, prefer source code and current test results; report
  the documentation drift.
- Run `make smoke` for meaningful kernel, MMU, scheduler, syscall, process, or
  I/O changes. A successful compile alone is not proof of completion.
- Do not change `STATUS.md` unless a milestone is completed and verified, or
  the user explicitly requests a documentation update.