---
stepsCompleted: ['step-01-discovery', 'step-02-classification', 'step-03-requirements', 'step-04-tools', 'step-05-plan-review', 'step-06-design', 'step-07-foundation', 'step-08-build-step-01', 'step-09-build-remaining-steps', 'step-10-confirmation', 'step-11-completion']
workflowName: epic-pipeline
created: 2026-03-12
completionDate: 2026-03-12
status: COMPLETE
confirmationDate: 2026-03-12
confirmationType: new_workflow
coverageStatus: complete
approvedDate: 2026-03-12
---

# Workflow Creation Plan

## Discovery Notes

**User's Vision:**
A fully autonomous "Epic Pipeline Orchestrator" that chains four existing implementation workflows — execute-epic, epic-review, retrospective, correct-course — in a continuous loop. It picks up the next non-done epic, runs the full cycle, applies learnings, and repeats until no epics remain. Zero human gates; YOLO mode throughout.

**Who It's For:**
Lucas (solo developer) — replaces the manual workflow-by-workflow invocation pattern with a single "run everything" command.

**What It Produces:**
No document output. This is an action-workflow (orchestrator) that invokes sub-workflows and tracks cycle state. The sub-workflows produce their own artifacts (PRs, retro docs, change proposals, etc.).

**Key Insights:**
- Orchestrator is the single source of truth for "current epic" — passes epic number/name explicitly to every sub-workflow. No sub-workflow should guess or auto-discover.
- Must self-heal out-of-sync state: cross-reference sprint-status.yaml, epics.md, and git state to determine true epic status.
- One epic per cycle, strictly sequential.
- Never halts on failure — retries indefinitely (CI failures, rebase conflicts, etc.).
- Passes retro doc path explicitly to correct-course so it knows what to apply.
- All sub-workflows run in YOLO mode (skip all human gates).

**Sub-Workflow Chain (per cycle):**
1. execute-epic (epic N) — develop stories, create PRs
2. epic-review (epic N) — merge PRs, rebase, resolve conflicts
3. retrospective (epic N) — generate retro doc
4. correct-course (retro doc from step 3) — apply learnings to planning artifacts
5. Loop → next epic

**Halt Condition:**
No more epics with status != "done" in sprint-status.yaml (cross-referenced with epics.md).

## Classification Decisions

**Workflow Name:** epic-pipeline
**Target Path:** _bmad/bmm/workflows/4-implementation/epic-pipeline/

**4 Key Decisions:**
1. **Document Output:** false (action-workflow / orchestrator)
2. **Module Affiliation:** BMM (Software Development — implementation phase)
3. **Session Type:** Continuable (multi-epic runs can span hours/sessions)
4. **Lifecycle Support:** Create-only (fixed orchestrator structure)

**Structure Implications:**
- Needs `steps-c/` only (no edit/validate modes)
- Needs `step-01b-continue.md` for resuming mid-pipeline after session interruption
- State tracked in a pipeline state file (not document frontmatter, since no document output)
- Pipeline state file must capture: current epic, current phase (execute/review/retro/correct-course), cycle count

## Requirements

**Flow Structure:**
- Pattern: Looping (INIT → DISCOVER → EXECUTE → REVIEW → RETRO → CORRECT-COURSE → LOOP)
- Phases: Init/Discover, Execute Epic, Review Epic, Retrospective + Correct-Course, Cycle Complete
- Estimated steps: 6 (step-01-init, step-01b-continue, step-02-execute, step-03-review, step-04-retro-and-correct, step-05-cycle-complete)

**User Interaction:**
- Style: Fully autonomous (zero human gates, YOLO mode)
- Decision points: None — all decisions are programmatic (state-based routing)
- Checkpoint frequency: Never pauses for input

**Inputs Required:**
- Required: sprint-status.yaml, epics.md, git branch state
- Required: Sub-workflow paths (execute-epic, epic-review, retrospective, correct-course)
- Optional: Existing pipeline-state.yaml (for resuming)
- Prerequisites: Planning artifacts must exist (PRD, architecture, epics with stories)

**Output Specifications:**
- Type: Action-workflow (orchestrator)
- Produces: pipeline-state.yaml (tracks current epic, phase, cycle count, completed epics history)
- No document output — sub-workflows produce their own artifacts
- Frequency: Continuous until all epics are done

**Success Criteria:**
- All non-done epics processed through full cycle (execute → review → retro → correct-course)
- Each sub-workflow invoked with explicit parameters (epic number, retro doc path, etc.)
- Pipeline state accurately reflects progress at all times
- Resumes correctly after session interruption (picks up at exact phase of exact epic)
- Never halts on failure — retries indefinitely
- Sprint-status.yaml consistent with actual state after each cycle

**Instruction Style:**
- Overall: Prescriptive (pure orchestration logic, no creative facilitation)
- Notes: Each step has exact instructions — invoke workflow X with parameters Y, check state Z, route to next step

## Tools Configuration

**Core BMAD Tools:**
- **Party Mode:** excluded — No creative facilitation in an orchestrator
- **Advanced Elicitation:** excluded — No human interaction
- **Brainstorming:** excluded — No ideation needed

**LLM Features:**
- **Web-Browsing:** excluded — All inputs are local files
- **File I/O:** included — Read/write pipeline-state.yaml, sprint-status.yaml, epics.md, retro docs
- **Sub-Agents:** excluded — Sequential execution, not parallel
- **Sub-Processes:** excluded — One workflow at a time

**Memory:**
- Type: Continuable (sidecar file)
- Tracking: pipeline-state.yaml — current epic number, current phase (execute/review/retro/correct-course), cycle count, completed epics history, timestamps

**External Integrations:**
- None beyond what sub-workflows already use (git, gh CLI, cargo)

**Installation Requirements:**
- None — all tools are built-in

## Detailed Design

### File Structure

```
epic-pipeline/
├── workflow.yaml
├── workflow.md
└── steps-c/
    ├── step-01-init.md
    ├── step-01b-continue.md
    ├── step-02-execute.md
    ├── step-03-review.md
    ├── step-04-retro-and-correct.md
    └── step-05-cycle-complete.md
```

### Step Specifications

#### step-01-init.md (Init, Continuable)
- **Goal:** Check for existing pipeline state, discover next non-done epic
- **Menu:** Auto-proceed (no human interaction)
- **Logic:**
  1. Check for existing pipeline-state.yaml
  2. If exists and currentPhase != "complete" → route to step-01b-continue
  3. If not exists or last epic complete → discover next epic:
     a. Read sprint-status.yaml — find all epics
     b. Read epics.md — cross-reference epic definitions
     c. Check git branches — detect actual state vs tracked state
     d. Find first epic where status != "done" (in epic order)
  4. If no epics remain → display "All epics complete!" and halt
  5. Create/update pipeline-state.yaml with: currentEpic, currentEpicName, currentPhase="execute"
  6. Auto-proceed to step-02-execute

#### step-01b-continue.md (Continuation)
- **Goal:** Resume pipeline at exact phase of interrupted epic
- **Menu:** Auto-proceed
- **Logic:**
  1. Read pipeline-state.yaml — get currentEpic, currentPhase
  2. Display: "Resuming pipeline — Epic {N}: {name}, phase: {phase}"
  3. Route to correct step based on currentPhase:
     - "execute" → step-02-execute
     - "review" → step-03-review
     - "retro" → step-04-retro-and-correct
     - "correct-course" → step-04-retro-and-correct (skip retro, go to correct-course)
     - "complete" → step-05-cycle-complete

#### step-02-execute.md (Middle, Simple)
- **Goal:** Invoke execute-epic for current epic
- **Menu:** Auto-proceed
- **Logic:**
  1. Read pipeline-state.yaml — get currentEpic, currentEpicName
  2. Display: "Executing Epic {N}: {name}"
  3. Invoke execute-epic workflow:
     - Pass: `epic {N}` explicitly
     - Mode: YOLO (skip all human gates)
     - Path: `{project-root}/_bmad/bmm/workflows/4-implementation/execute-epic/workflow.yaml`
  4. Update pipeline-state.yaml: currentPhase = "review"
  5. Auto-proceed to step-03-review

#### step-03-review.md (Middle, Simple)
- **Goal:** Invoke epic-review for current epic
- **Menu:** Auto-proceed
- **Logic:**
  1. Read pipeline-state.yaml — get currentEpic, currentEpicName
  2. Display: "Reviewing Epic {N}: {name}"
  3. Invoke epic-review workflow:
     - Pass: `epic {N}` explicitly
     - Mode: YOLO (merge PRs autonomously, no human review)
     - Path: `{project-root}/_bmad/bmm/workflows/4-implementation/epic-review/workflow.yaml`
  4. Update pipeline-state.yaml: currentPhase = "retro"
  5. Auto-proceed to step-04-retro-and-correct

#### step-04-retro-and-correct.md (Middle, Simple)
- **Goal:** Invoke retrospective then correct-course for current epic
- **Menu:** Auto-proceed
- **Logic:**
  1. Read pipeline-state.yaml — get currentEpic, currentEpicName, currentPhase
  2. **If currentPhase == "retro":**
     a. Display: "Running retrospective for Epic {N}: {name}"
     b. Invoke retrospective workflow:
        - Pass: `epic {N}` explicitly
        - Mode: YOLO
        - Path: `{project-root}/_bmad/bmm/workflows/4-implementation/retrospective/workflow.yaml`
     c. Update pipeline-state.yaml: currentPhase = "correct-course"
  3. **Locate retro doc:**
     - Search: `{implementation_artifacts}/epic-{N}-retro-*.md`
     - Use the most recent match
  4. Display: "Running correct-course for Epic {N} with retro: {retro_doc_path}"
  5. Invoke correct-course workflow:
     - Pass: `epic {N}` explicitly + retro doc path
     - Mode: YOLO
     - Path: `{project-root}/_bmad/bmm/workflows/4-implementation/correct-course/workflow.yaml`
  6. Update pipeline-state.yaml: currentPhase = "complete"
  7. Auto-proceed to step-05-cycle-complete

#### step-05-cycle-complete.md (Loop-back)
- **Goal:** Finalize cycle, loop to next epic
- **Menu:** Auto-proceed (loops)
- **Logic:**
  1. Read pipeline-state.yaml
  2. Add current epic to completedEpics array with timestamp
  3. Increment cycleCount
  4. Display: "Cycle {N} complete — Epic {epicNumber}: {epicName} done"
  5. Display cumulative summary: total cycles, total epics completed
  6. Loop back: load and execute step-01-init to discover next epic

### State File: pipeline-state.yaml

```yaml
startedAt: "2026-03-12T..."
currentEpic: 12
currentEpicName: "Templates & Variable Rendering"
currentPhase: "execute"
cycleCount: 1
completedEpics:
  - epicNumber: 11
    epicName: "Multi-Format Output (PNG/WebP/JPEG)"
    completedAt: "2026-03-12T..."
```

### Role & Persona

Pipeline controller — disciplined, prescriptive, state-driven. No creative facilitation. Reads state, invokes workflows, updates state, loops.

### Workflow Chaining — Explicit Parameter Passing

| Sub-Workflow | Explicit Parameters |
|-------------|-------------------|
| execute-epic | epic number, epic name |
| epic-review | epic number, epic name, YOLO mode |
| retrospective | epic number, epic name, YOLO mode |
| correct-course | epic number, retro doc path, YOLO mode |

### Error Handling

- Never halts on failure — sub-workflows handle their own retries
- If sub-workflow returns without completing, orchestrator retries the same phase
- Pipeline state always reflects current position for safe resumption
