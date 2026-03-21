---
name: 'step-05-cycle-complete'
description: 'Finalize cycle, record completed epic, loop back to discover next epic'

initStepFile: './step-01-init.md'
pipelineStateFile: '{output_folder}/pipeline-state.yaml'
---

# Step 5: Cycle Complete

## STEP GOAL:

To finalize the current epic cycle — record the completed epic in the pipeline state history, increment the cycle count, display a cumulative summary, and loop back to step-01-init to discover the next epic.

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

- 🎯 Focus ONLY on recording completion and looping back
- 🚫 FORBIDDEN to invoke any sub-workflow in this step
- 🚫 FORBIDDEN to halt for user input
- 🚫 FORBIDDEN to skip the loop-back — always return to step-01-init
- 💬 Auto-proceed after recording completion

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update pipeline-state.yaml with completed epic and cycle count
- 📖 Display cumulative summary for visibility
- 🚫 FORBIDDEN to end the pipeline here — always loop back

## CONTEXT BOUNDARIES:

- Available: pipeline-state.yaml with currentEpic, currentEpicName, currentPhase="complete"
- Focus: Record completion, loop back
- Limits: Do not discover the next epic — step-01-init handles that
- Dependencies: step-04 must have set currentPhase="complete"

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Pipeline State

Load {pipelineStateFile} and extract:

- `currentEpic` — the epic number just completed
- `currentEpicName` — the epic name
- `cycleCount` — current cycle number
- `completedEpics` — array of previously completed epics

### 2. Record Completed Epic

Update {pipelineStateFile}:

- Add to `completedEpics` array:
  ```yaml
  - epicNumber: [currentEpic]
    epicName: "[currentEpicName]"
    completedAt: "[current ISO 8601 timestamp]"
  ```
- Increment `cycleCount` by 1
- Keep `currentEpic` and `currentEpicName` as-is (for the summary)
- Set `currentPhase` to `"complete"` (confirms finalization)

### 3. Display Cycle Summary

Display:

```
**Cycle {cycleCount} complete — Epic {currentEpic}: {currentEpicName} done.**

**Pipeline Summary:**
- Total cycles completed: {cycleCount}
- Epics completed this run: {list all completedEpics with epicNumber and epicName}
- Pipeline started: {startedAt}
```

### 4. Loop Back to Discover Next Epic

Display: "**Looping back to discover next epic...**"

#### Menu Handling Logic:

- After completion is recorded, immediately load, read entire file, then execute {initStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- ALWAYS loop back to step-01-init — never end the pipeline here
- Step-01-init will determine if there are more epics or if the pipeline should halt

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state read correctly
- Completed epic added to history with timestamp
- Cycle count incremented
- Cumulative summary displayed
- Looped back to step-01-init

### ❌ SYSTEM FAILURE:

- Not recording the completed epic in the history
- Not incrementing cycle count
- Ending the pipeline instead of looping back
- Halting for user input
- Skipping the cumulative summary

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
