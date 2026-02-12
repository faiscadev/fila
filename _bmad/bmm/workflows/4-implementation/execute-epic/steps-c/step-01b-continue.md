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
  - If not found (all completed or skipped) → load, read entirely, then execute {epicCompleteStep}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Route directly to the appropriate step based on state

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- State file read and parsed correctly
- Progress summary displayed
- Routed to the correct step based on state
- No completed work re-executed

### ❌ SYSTEM FAILURE:

- State file not found or unreadable
- Re-executing completed stories
- Routing to wrong step for current phase
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
