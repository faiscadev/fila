---
name: 'step-03-review'
description: 'Invoke epic-review sub-workflow to merge PRs for the current epic'

nextStepFile: './step-04-retro-and-correct.md'
pipelineStateFile: '{output_folder}/pipeline-state.yaml'
epicReviewWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/epic-review/workflow.yaml'
---

# Step 3: Review Epic

## STEP GOAL:

To invoke the epic-review sub-workflow for the current epic, passing the epic number and name explicitly with YOLO mode. The review workflow will merge all PRs, rebase, and resolve conflicts autonomously. Upon completion, update pipeline state to the "retro" phase.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are an autonomous pipeline controller
- ✅ You execute prescriptive instructions methodically and precisely
- ✅ There are no human checkpoints — you run from start to finish
- ✅ Zero tolerance for skipped steps or incomplete tracking

### Step-Specific Rules:

- 🎯 Focus ONLY on invoking epic-review and updating pipeline state
- 🚫 FORBIDDEN to review code yourself — the sub-workflow handles that
- 🚫 FORBIDDEN to halt for user input
- 🚫 FORBIDDEN to let the sub-workflow auto-discover the epic
- 💬 YOLO mode: merge autonomously, no human review gates

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update pipeline-state.yaml after sub-workflow completes
- 📖 Pass ALL parameters explicitly to the sub-workflow
- 🚫 FORBIDDEN to manually merge PRs — the sub-workflow handles that

## CONTEXT BOUNDARIES:

- Available: pipeline-state.yaml with currentEpic, currentEpicName, currentPhase="review"
- Focus: Invoke epic-review to merge all PRs for the current epic
- Limits: Do not run retrospective — that's the next phase
- Dependencies: step-02-execute must have completed (PRs exist to review)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Pipeline State

Load {pipelineStateFile} and extract:

- `currentEpic` — the epic number to review
- `currentEpicName` — the epic name for display and parameter passing

Display: "**Reviewing Epic {currentEpic}: {currentEpicName}**"

### 2. Invoke Epic-Review Sub-Workflow

Load and execute the epic-review workflow at {epicReviewWorkflow}.

**Explicit parameters to pass:**

- **Epic:** `epic {currentEpic}` — the review workflow must know which epic's PRs to merge
- **Mode:** YOLO — merge all PRs autonomously, no human review, rebase and resolve conflicts
- **Instruction:** "Review and merge all PRs for epic {currentEpic}: {currentEpicName}. YOLO mode — merge autonomously without human review. Rebase onto main, resolve any conflicts, and merge each PR in story order."

**CRITICAL:** The epic-review workflow MUST receive the epic number explicitly. It must merge PRs autonomously without stopping for human review.

**On completion:** All story PRs for the epic should be merged to main. Story statuses updated to "done" in sprint-status.yaml. Epic status updated to "done".

**On failure/incomplete:** If the sub-workflow exits without merging all PRs (conflicts it can't resolve, CI failures), note the issue but proceed — the retrospective will capture what happened, and correct-course can adjust the plan.

### 3. Update Pipeline State

Update {pipelineStateFile}:

- Set `currentPhase` to `"retro"`

Display: "**Epic {currentEpic} review complete. Advancing to retrospective phase.**"

### 4. Auto-Proceed to Retrospective Phase

Display: "**Proceeding to retrospective for Epic {currentEpic}...**"

#### Menu Handling Logic:

- After pipeline state is updated, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after review completes

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state read correctly
- Epic-review sub-workflow invoked with explicit epic number and YOLO mode
- PRs merged autonomously (no human gates)
- Pipeline state updated to currentPhase="retro"
- Auto-proceeded to step 04

### ❌ SYSTEM FAILURE:

- Not reading pipeline state before invoking sub-workflow
- Letting epic-review auto-discover the epic
- Stopping for human PR review (violates YOLO mode)
- Not updating pipeline state after completion
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
