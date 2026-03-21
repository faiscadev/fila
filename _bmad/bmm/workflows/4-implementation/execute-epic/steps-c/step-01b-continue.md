---
name: 'step-01b-continue'
description: 'Resume an interrupted epic execution from where it left off'

stateFile: '{output_folder}/epic-execution-state.yaml'
storySetupStep: './step-02-story-setup.md'
storyDevStep: './step-03-story-dev.md'
codeReviewStep: './step-04-code-review.md'
prCiStep: './step-05-pr-ci.md'
storyCompleteStep: './step-06-story-complete.md'
epicCompleteStep: './step-07-epic-complete.md'
---

# Step 1b: Continue Epic Execution

## STEP GOAL:

To resume an interrupted epic execution by reading the state file, determining where execution left off, and routing to the appropriate step.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are an autonomous execution engine resuming a previous run
- ✅ You execute prescriptive instructions methodically and precisely
- ✅ There are no human checkpoints — you resume and run to completion
- ✅ Zero tolerance for skipped steps or incomplete tracking

### Step-Specific Rules:

- 🎯 Focus ONLY on determining where to resume
- 🚫 FORBIDDEN to re-execute completed stories
- 🚫 FORBIDDEN to halt for user input
- 💬 Route to the correct step based on state

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Read state file to determine progress
- 📖 Route to correct step based on currentPhase
- 🚫 FORBIDDEN to start from scratch if state exists

## CONTEXT BOUNDARIES:

- Available: Execution state file from previous run
- Focus: Resumption logic only
- Limits: Do not re-execute completed work
- Dependencies: State file must exist (routed here from step-01)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Execution State

Load {stateFile} and parse:

- Which stories are completed
- Which stories are skipped (and why)
- Which story is in-progress (if any)
- What phase the in-progress story is in (currentPhase)

### 1b. Cross-Reference Sprint Status (Stale State Detection)

Load sprint-status.yaml and find the epic's entry (e.g., `epic-4`, `epic-5`).

- **If the epic is marked `done` in sprint-status but the state file has stories that are NOT completed/skipped:**
  - The state file is stale — the epic was completed in a previous session but state tracking was lost.
  - For each in-progress or pending story: verify via `gh pr view` that its branch PR was merged. If merged, auto-correct the story status to `completed` and populate the PR number. If no PR exists or it was not merged, mark the story as `skipped` with reason "stale — epic already done per sprint-status".
  - Save the corrected state file.
  - Add `completedAt` with the current date/time to the state file.
  - Display: "**Stale state detected — sprint-status already marks this epic as done. Auto-corrected [N] stories. Looking for next epic...**"
  - Route to the "completedAt exists" branch in step 3 (look up next epic in backlog).

### 2. Display Progress Summary

Display:
"**Resuming epic execution.**

**Progress:**
- Completed: [N] stories
- Skipped: [N] stories
- Remaining: [N] stories
- Current: [story ID] - [title] (phase: [currentPhase])"

### 3. Route to Correct Step

Based on the state:

- **If a story has status "in-progress":**
  - currentPhase is empty or "setup" → load, read entirely, then execute {storySetupStep}
  - currentPhase is "dev" → load, read entirely, then execute {storyDevStep}
  - currentPhase is "code-review" → load, read entirely, then execute {codeReviewStep}
  - currentPhase is "pr-ci" → load, read entirely, then execute {prCiStep}

- **If no story is "in-progress":** Find the first story with status "pending":
  - If found → load, read entirely, then execute {storySetupStep}
  - If not found (all completed or skipped) AND the state file has a `completedAt` field (epic was already finalized in a previous session):
    - Look up sprint-status.yaml to find the next epic in `backlog` status
    - If a next epic exists → delete the current state file, display "**Previous epic already complete. Starting next epic: [epic name]...**", then re-execute step-01-init.md (which will create a fresh state file for the new epic)
    - If no next epic in backlog → display "**All epics complete. Nothing to execute.**" and halt
  - If not found (all completed or skipped) AND no `completedAt` field (epic just finished in this session) → load, read entirely, then execute {epicCompleteStep}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Route directly to the appropriate step based on state

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- State file read and parsed correctly
- Sprint-status cross-referenced to detect stale state
- Stale stories auto-corrected when epic already done in sprint-status
- Progress summary displayed
- Routed to the correct step based on state
- No completed work re-executed

### ❌ SYSTEM FAILURE:

- State file not found or unreadable
- Re-executing completed stories
- Routing to wrong step for current phase
- Halting for user input
- Ignoring sprint-status when state file is stale (trying to dev an already-merged story)

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
