# Archive

Documents archived because the architecture they reference no longer exists.

## Why archived

These documents were written for the **Extension Actor + EventBus** architecture,
which was removed in v0.1.x. The `extensions` crate, `ExtensionActor`, `HookRouter`,
and `EventBus` no longer exist. Built-in strategies (audit, path_guard, tool_guard,
token_budget) are now inlined in `agent-core::hook::DefaultHookDispatcher`. Hook calls
are direct function calls (no Actor, no EventBus).

See [AGENTS.md](../../AGENTS.md) (ADR-002, ADR-003) for the current architecture.

## Contents

| Document | Original location |
|---|---|
| `plans/2026-05-03-agent-core-implementation.md` | `docs/plans/` |
| `plans/2026-05-03-joint-development-roadmap.md` | `docs/plans/` |
| `plans/prompt-builder-impl.md` | `docs/plans/` |
| `specs/2026-05-02-agent-core.md` | `docs/specs/` |
| `specs/prompt-builder.md` | `docs/specs/` |
| `superpowers-plans/2026-05-04-tenant-core.md` | `docs/superpowers/plans/` |
| `superpowers-plans/2026-05-11-content-filter-fixes.md` | `docs/superpowers/plans/` |
