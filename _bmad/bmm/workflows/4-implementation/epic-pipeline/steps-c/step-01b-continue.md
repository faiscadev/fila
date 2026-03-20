---
name: 'step-01b-continue'
description: 'Resume pipeline at exact phase of interrupted epic'

pipelineStateFile: '{output_folder}/pipeline-state.yaml'
executeStepFile: './step-02-execute.md'
reviewStepFile: './step-03-review.md'
retroStepFile: './step-04-retro-and-correct.md'
cycleCompleteStepFile: './step-05-cycle-complete.md'
---

# Step 1b: Continue Pipeline

## STEP GOAL:

To resume the pipeline from where it was interrupted in a previous session. Read the pipeline state, determine the exact epic and phase, and route to the correct step.

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

- 🎯 Focus ONLY on reading state and routing to the correct step
- 🚫 FORBIDDEN to invoke any sub-workflow in this step
- 🚫 FORBIDDEN to halt for user input
- 💬 Auto-proceed once routing is determined

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Do not modify pipeline-state.yaml — just read and route
- 📖 Log the resumption for visibility
- 🚫 FORBIDDEN to skip state reading

## CONTEXT BOUNDARIES:

- Available: Existing pipeline-state.yaml from previous session
- Focus: Read state, determine phase, route to correct step
- Limits: Do not re-discover epics or modify state
- Dependencies: pipeline-state.yaml must exist (step-01-init routes here only if it does)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Pipeline State

Load {pipelineStateFile} and extract:

- `currentEpic` — the epic number being processed
- `currentEpicName` — the human-readable epic name
- `currentPhase` — the phase that was in progress when interrupted
- `cycleCount` — which cycle we're on
- `completedEpics` — history of completed epics

### 2. Display Resumption Status

Display: "**Resuming pipeline — Epic {currentEpic}: {currentEpicName}, phase: {currentPhase} (cycle {cycleCount})**"

If `completedEpics` has entries, display: "**Previously completed: {count} epics**"

### 3. Route to Correct Step

Based on `currentPhase`, load the appropriate step file:

| currentPhase | Route To | Notes |
|-------------|----------|-------|
| `execute` | {executeStepFile} | Resume/restart epic execution |
| `review` | {reviewStepFile} | Resume/restart epic review |
| `retro` | {retroStepFile} | Run retrospective then correct-course |
| `correct-course` | {retroStepFile} | Skip retro, run only correct-course (step-04 handles this) |
| `complete` | {cycleCompleteStepFile} | Finalize the cycle |

Display: "**Routing to {currentPhase} phase...**"

Load, read entire file, then execute the appropriate step file from the table above.

#### Menu Handling Logic:

- After routing is determined, immediately load, read entire file, then execute the target step file

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to the routed step after state reading

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state read correctly
- Resumption status displayed
- Routed to correct step based on currentPhase
- No state modification (read-only)

### ❌ SYSTEM FAILURE:

- Not reading pipeline state before routing
- Routing to wrong step for the current phase
- Modifying pipeline state in this step
- Halting for user input
- Re-discovering epics instead of using saved state

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
