---
stepsCompleted: ['step-01-discovery', 'step-02-classification', 'step-03-requirements', 'step-04-tools', 'step-05-plan-review', 'step-06-design', 'step-07-foundation', 'step-08-build-step-01', 'step-09-build-next-step', 'step-10-confirmation', 'step-11-completion']
created: 2026-02-13
completionDate: 2026-02-13
status: COMPLETE
---

# Workflow Creation Plan: epic-review

## Discovery Notes

**User's Vision:**
A structured, human-in-the-loop workflow for the post-execute-epic PR review/merge/rebase cycle. Solves the problem of ad-hoc review sessions bypassing workflow gates — which caused cascading incidents in Epic 4 (wrong PR targets, missed Cubic reviews, corrupt sprint-status during rebases).

**Who It's For:**
The agent (Claude Code) collaborating with Lucas during the review phase after execute-epic completes. Lucas reviews/merges PRs on GitHub; the agent handles code changes, rebases, CI/Cubic gates, and status tracking.

**What It Produces:**
All PRs in a stacked chain reviewed, addressed, and merged — with sprint-status.yaml and story files updated to "done". No output document; the workflow drives a process.

**Key Insights:**
- Agent discovers the PR chain from sprint-status.yaml and git state (user doesn't provide it)
- Two user actions per PR: "submitted a review" (agent addresses findings, loops) or "merged" (agent rebases next branch)
- Cascade rebase conflict resolution must enforce *reasoning about correct state*, not mechanical auto-resolution
- Cubic gates are hard gates — never skip
- Story file status updates ("done") and sprint-status updates are part of the workflow

## Classification Decisions

**Workflow Name:** epic-review
**Target Path:** `_bmad/bmm/workflows/4-implementation/epic-review/`

**4 Key Decisions:**
1. **Document Output:** false
2. **Module Affiliation:** BMM (software development, 4-implementation phase)
3. **Session Type:** continuable
4. **Lifecycle Support:** create-only

## Requirements

**Flow Structure:**
- Pattern: looping (outer loop per PR, inner loop per review cycle)
- Phases: init → review loop → rebase & transition → finalization

**User Interaction:**
- Style: mixed — prescriptive process gates, autonomous code/rebase work
- Decision points: only "submitted a review" vs "merged" per PR
- User does NOT participate in conflict resolution

**Inputs Required:**
- sprint-status.yaml, execute-epic state file, story implementation artifact files

**Success Criteria:**
- All PRs merged, all story artifacts "done", sprint-status reflects completed epic

**Instruction Style:**
- Prescriptive process, intent-based code changes

## Tools Configuration

**Core BMAD Tools:**
- Party Mode: excluded — not applicable for prescriptive review cycle
- Advanced Elicitation: excluded — not applicable
- Brainstorming: excluded — not applicable

**LLM Features:**
- File I/O: included — reading/writing state files, story artifacts, sprint-status
- Web-Browsing: excluded — not needed
- Sub-Agents: excluded — not needed
- Sub-Processes: excluded — not needed

**Memory:**
- Type: continuable via sidecar state file (epic-review-state.yaml)
- Tracking: per-PR status in state file (pending/reviewing/merged)

## Design

**Step Structure:**

| Step | Name | Type | Purpose |
|------|------|------|---------|
| 01 | Init | Continuable Init | Discover PR chain, create state file |
| 01b | Continue | Continuation | Resume interrupted session |
| 02 | Review PR | Middle (Custom Menu) | Present PR, handle R/M signals, address findings |
| 03 | Rebase Next | Middle (Auto-proceed) | Rebase next branch, resolve conflicts, CI/Cubic gate |
| 04 | Finalize | Final | Update sprint-status, story files, summary |

**Flow:**
```
step-01 → step-02 ←→ (inner loop: R signal)
              ↓ (M signal, more PRs)
          step-03 → step-02
              ↓ (M signal, last PR)
          step-04
```

## Build Summary

**Files Created:**
- `workflow.md` — entry point with architecture and initialization
- `steps-c/step-01-init.md` — PR chain discovery from sprint-status + execute-epic state
- `steps-c/step-01b-continue.md` — session resumption
- `steps-c/step-02-review-pr.md` — main review loop with R/M menu and CI/Cubic gate
- `steps-c/step-03-rebase-next.md` — cascade rebase with conflict resolution reasoning protocol
- `steps-c/step-04-finalize.md` — sprint-status and story artifact updates
