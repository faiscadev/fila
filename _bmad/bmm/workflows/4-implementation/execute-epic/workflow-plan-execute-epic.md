---
workflowName: execute-epic
stepsCompleted: ['step-01-discovery', 'step-02-classification', 'step-03-requirements', 'step-04-tools', 'step-05-plan-review', 'step-06-design', 'step-07-foundation', 'step-08-build-step-01', 'step-09-build-all-steps', 'step-10-confirmation', 'step-11-completion']
created: 2026-02-11
approvedDate: 2026-02-11
completionDate: 2026-02-11
status: COMPLETE
---

# Workflow Creation Plan

## Discovery Notes

**User's Vision:**
A single-command workflow that takes a planned epic and autonomously executes every story in it — from creating detailed story specs through to opening clean, green PRs. The workflow enforces all bookkeeping that was skipped when done manually in Epic 1 (story file updates, sprint status, dev agent records, test coverage verification).

**Who It's For:**
Lucas (project lead), running it personally to execute epics end-to-end.

**What It Produces:**
A set of open, green PRs (one per story) with all tracking artifacts updated, ready for human review.

**Key Insights:**
- Born from Epic 1 retrospective where manual step-chaining led to 6/9 stories with broken tracking
- Agents optimized for throughput over process — baking bookkeeping into automation prevents skipping
- Must orchestrate existing BMAD skills (bmad-bmm-create-story, bmad-bmm-dev-story, bmad-bmm-code-review)
- Dependency analysis needed to determine parallel vs sequential story execution
- No human checkpoints — runs fully autonomously until all PRs are open and green
- Automated PR review feedback (from GitHub review agents) must be addressed as part of the cycle
- Per-story cycle: create story spec -> develop -> code review -> update tracking -> open PR -> handle CI + review feedback -> next story

## Classification Decisions

**Workflow Name:** execute-epic
**Target Path:** `_bmad/bmm/workflows/4-implementation/execute-epic/`

**4 Key Decisions:**
1. **Document Output:** false — orchestrates actions and existing skills, doesn't produce its own document
2. **Module Affiliation:** BMM — chains BMM skills (create-story, dev-story, code-review)
3. **Session Type:** Continuable — epics with multiple stories will consume massive tokens, needs resume capability
4. **Lifecycle Support:** Create-only — execution workflow with no output document to edit or validate

**Structure Implications:**
- Needs `steps-c/` only (create-only)
- Needs `step-01b-continue.md` for resuming interrupted epic execution
- Needs `stepsCompleted` tracking in frontmatter for continuation
- No document template needed (non-document workflow)

## Requirements

**Flow Structure:**
- Pattern: Repeating with nested inner loop
- Outer loop: Repeat full cycle for each story in the epic
- Inner loop 1: Code review iteration until clean
- Inner loop 2: CI + automated PR review iteration until green
- Phases: (1) Dependency analysis & ordering, (2) Per-story execution cycle
- Per-story cycle: create story spec → develop → code review ↔ iterate → open PR → CI + PR review ↔ iterate → update tracking → next story

**User Interaction:**
- Style: Mostly autonomous
- Decision points: None — no human checkpoints
- Checkpoint frequency: None — runs until all PRs are open and green

**Inputs Required:**
- Required: Epic plan file (stories, order, dependencies)
- Required: Sprint status file
- Optional: Base branch (default: main)
- Discovered: Everything else from BMM module conventions and existing BMAD skills

**Output Specifications:**
- Type: Actions + artifacts (no single output document)
- Per story: feature branch, story spec file, implemented code with tests, updated story file (status, dev agent record, task checkboxes), open PR
- Per epic: updated sprint-status.yaml, all PRs open and green

**Success Criteria:**
- All stories in the epic have open, green PRs
- Every story file updated: status → done, dev agent record filled, task checkboxes checked
- Sprint status up to date
- Test coverage verified during code review for every story
- No skipped bookkeeping steps

**Failure Conditions:**
- Story stuck in review loop that can't resolve
- Story tracking files left incomplete
- CI failures not addressed

**Instruction Style:**
- Overall: Prescriptive
- Notes: Fully autonomous execution engine — exact instructions, no room for creative interpretation

## Tools Configuration

**Core BMAD Tools:**
- **Party Mode:** excluded — no human interaction, autonomous execution
- **Advanced Elicitation:** excluded — no human to elicit from
- **Brainstorming:** excluded — not applicable to execution workflow

**LLM Features:**
- **Web-Browsing:** excluded — operates on local files and GitHub only
- **File I/O:** included — reads epic plans, writes story files, updates sprint status
- **Sub-Agents:** included — invokes existing BMAD skills (create-story, dev-story, code-review)
- **Sub-Processes:** excluded — sequential execution for now, parallel can be added later

**Memory:**
- Type: Continuable
- Tracking: stepsCompleted (which stories are done), lastStep (where within story cycle), step-01b-continue.md for resumption
- State: per-story completion status, current position in cycle

**External Integrations:**
- GitHub (gh CLI) — opening PRs, checking CI status, reading automated review feedback. Already available via bash, no MCP needed.

**Installation Requirements:**
- None — all tools are built-in or already available

## Workflow Design

### Key Design Decisions

1. **Skills as reference documents** — The workflow loads existing BMAD skills (create-story, dev-story, code-review) as reference/context and executes their process autonomously. Skills are NOT invoked interactively. This ensures the workflow benefits from the skills' latest knowledge while running fully hands-free.

2. **Dedicated state file** — Continuation state is tracked in a YAML file (`_bmad-output/epic-execution-state.yaml`) with per-story status, current phase, branch names, and PR numbers. On resume, the workflow reads this file to determine where to pick up.

3. **Skip and log on failure** — If a story gets stuck in a review/CI loop (max iterations exceeded), the workflow logs the issue, marks the story as skipped, and moves to the next story. One stuck story doesn't block the epic.

4. **No subprocess optimization** — Sequential by nature. Parallel story execution deferred to future enhancement.

5. **No menus** — All steps auto-proceed (Pattern 3). Fully autonomous, no A/P options.

### File Structure

```
execute-epic/
├── workflow.md                   # Entry point, config, routing
├── steps-c/
│   ├── step-01-init.md           # Parse epic, build execution plan, detect continuation
│   ├── step-01b-continue.md      # Resume interrupted execution
│   ├── step-02-story-setup.md    # Branch + create story spec (refs create-story skill)
│   ├── step-03-story-dev.md      # Implement story (refs dev-story skill)
│   ├── step-04-code-review.md    # Review loop (refs code-review skill)
│   ├── step-05-pr-ci.md          # PR, CI, automated review loop + update tracking
│   ├── step-06-story-complete.md # Mark complete, loop back to step-02 or proceed to step-07
│   └── step-07-epic-complete.md  # Final verification + summary
└── data/
    └── (reference data if needed)
```

### Step Details

#### step-01-init.md (Init — Continuable)
- **Goal:** Parse the epic plan, build story execution order, initialize state
- **Type:** Init (Continuable) — checks for existing state file
- **Actions:**
  1. Load config
  2. Locate epic plan file and sprint status
  3. Parse stories and their dependencies
  4. Build execution order (topological sort by dependencies)
  5. Check for existing state file → if found, route to step-01b-continue
  6. Create state file with all stories as "pending"
  7. Auto-proceed to step-02
- **Menu:** Auto-proceed (Pattern 3)

#### step-01b-continue.md (Continuation)
- **Goal:** Resume an interrupted epic execution
- **Type:** Continuation (01b)
- **Actions:**
  1. Read state file
  2. Identify first non-completed story
  3. Determine where in the cycle it was interrupted (currentPhase)
  4. Welcome back with progress summary
  5. Route to the appropriate step (step-02 through step-05 based on currentPhase)
- **Menu:** Auto-proceed (Pattern 3)

#### step-02-story-setup.md (Middle — Simple)
- **Goal:** Set up branch and create detailed story spec
- **Type:** Middle (Simple)
- **Actions:**
  1. Read current story from state file
  2. Create feature branch (from base branch or dependent story's branch)
  3. Load bmad-bmm-create-story workflow as reference
  4. Execute its process autonomously — create story spec file with ACs, dev notes, tasks
  5. Update state file: currentPhase → "setup complete"
  6. Auto-proceed to step-03
- **Menu:** Auto-proceed (Pattern 3)
- **Skill reference:** bmad-bmm-create-story

#### step-03-story-dev.md (Middle — Simple)
- **Goal:** Implement the story
- **Type:** Middle (Simple)
- **Actions:**
  1. Load bmad-bmm-dev-story workflow as reference
  2. Execute its process autonomously — implement code, write tests, commit
  3. Update state file: currentPhase → "dev complete"
  4. Auto-proceed to step-04
- **Menu:** Auto-proceed (Pattern 3)
- **Skill reference:** bmad-bmm-dev-story

#### step-04-code-review.md (Middle — Loop)
- **Goal:** Review code until clean
- **Type:** Middle with inner loop
- **Actions:**
  1. Load bmad-bmm-code-review workflow as reference
  2. Execute review autonomously — verify tests cover all ACs
  3. If issues found: fix, commit, re-review (inner loop)
  4. Max iterations: if exceeded, log issue, mark story as skipped, jump to step-06
  5. When clean: update state file: currentPhase → "review complete"
  6. Auto-proceed to step-05
- **Menu:** Auto-proceed (Pattern 3)
- **Skill reference:** bmad-bmm-code-review
- **Error handling:** Max iteration guard, skip and log

#### step-05-pr-ci.md (Middle — Loop)
- **Goal:** Open PR, get CI + automated reviews green, update tracking
- **Type:** Middle with inner loop
- **Actions:**
  1. Update story file: status → done, dev agent record, task checkboxes
  2. Update sprint-status.yaml
  3. Open PR via gh CLI
  4. Wait for CI + automated PR review agents
  5. If feedback: address, push, wait again (inner loop)
  6. Max iterations: if exceeded, log issue, mark story as skipped, jump to step-06
  7. When green: update state file: currentPhase → "pr complete"
  8. Auto-proceed to step-06
- **Menu:** Auto-proceed (Pattern 3)
- **Error handling:** Max iteration guard, skip and log

#### step-06-story-complete.md (Loop Control)
- **Goal:** Mark story complete, decide next action
- **Type:** Loop control step
- **Actions:**
  1. Mark current story as "completed" (or "skipped") in state file
  2. Check if more stories remain
  3. If yes: set next story as current, route back to step-02
  4. If no: auto-proceed to step-07
- **Menu:** Auto-proceed (Pattern 3)

#### step-07-epic-complete.md (Final)
- **Goal:** Final verification and summary
- **Type:** Final step
- **Actions:**
  1. Read state file for complete picture
  2. Verify all PRs are open and green (via gh CLI)
  3. Print summary: completed stories, skipped stories (with reasons), PR links
  4. No next step — workflow complete
- **Menu:** None (final step)

### State File Format

```yaml
epic: epic-2
baseBranch: main
stateFile: _bmad-output/epic-execution-state.yaml
stories:
  - id: "2.1"
    status: completed    # pending | in-progress | completed | skipped
    currentPhase: ""     # setup | dev | code-review | pr-ci
    branch: feat/story-2.1
    pr: 42
  - id: "2.2"
    status: in-progress
    currentPhase: code-review
    branch: feat/story-2.2
    pr: null
  - id: "2.3"
    status: pending
    currentPhase: ""
    branch: ""
    pr: null
skippedIssues:
  - story: "2.1"
    phase: code-review
    reason: "Review loop exceeded max iterations (5)"
    iteration: 5
```

### Data Flow

```
Epic Plan → step-01 (parse) → State File
                                   ↓
step-02 (branch + story spec) ← State File → step-03 (develop)
                                                    ↓
step-04 (review loop) → step-05 (PR + CI loop) → step-06 (complete/loop)
                                                    ↓
                                              step-07 (summary)
```

### Role/Persona
- Autonomous execution engine
- Prescriptive, methodical
- No personality — it's a machine running a runbook

### Error Handling
- Code review loop: max iterations (configurable), skip and log on exceed
- CI/PR review loop: max iterations (configurable), skip and log on exceed
- If story skipped: logged in state file with reason, included in final summary
