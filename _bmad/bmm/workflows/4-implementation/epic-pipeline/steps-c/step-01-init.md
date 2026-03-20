---
name: 'step-01-init'
description: 'Check for existing pipeline state, discover next non-done epic, initialize pipeline'

nextStepFile: './step-02-execute.md'
continueFile: './step-01b-continue.md'
pipelineStateFile: '{output_folder}/pipeline-state.yaml'
sprintStatusFile: '{implementation_artifacts}/sprint-status.yaml'
epicsFile: '{planning_artifacts}/epics.md'
---

# Step 1: Initialize Pipeline

## STEP GOAL:

To check for existing pipeline state (for resumption), discover the next non-done epic by cross-referencing sprint-status.yaml and epics.md, and initialize pipeline-state.yaml. If a previous pipeline run was interrupted mid-epic, route to continuation.

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

- 🎯 Focus ONLY on discovering the next epic and initializing state
- 🚫 FORBIDDEN to invoke any sub-workflow in this step
- 🚫 FORBIDDEN to halt for user input
- 💬 Auto-proceed once initialization is complete

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Create or update pipeline-state.yaml with discovered epic
- 📖 Cross-reference multiple sources to determine true epic status
- 🚫 FORBIDDEN to skip state verification

## CONTEXT BOUNDARIES:

- Available: BMM config, sprint-status.yaml, epics.md, git state
- Focus: Epic discovery and state initialization
- Limits: Do not begin executing — only discover and set up state
- Dependencies: None — this is the first step

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Check for Existing Pipeline State

Look for an existing pipeline state file at {pipelineStateFile}.

- **If the file exists and `currentPhase` is NOT "complete" and NOT empty:** A previous pipeline run was interrupted mid-epic. Load, read entirely, then execute {continueFile} to resume.
- **If the file does not exist, or the last epic's phase is "complete":** Continue to step 2 to discover the next epic.

### 2. Read Sprint Status

Load and parse {sprintStatusFile}. Extract all epic entries and their statuses. An epic entry follows the pattern `epic-N: <status>` where status is one of: `backlog`, `in-progress`, or `done`.

### 3. Read Epics File

Load and parse {epicsFile}. Extract epic numbers, names, and story lists. This provides the human-readable epic names and the story breakdown for each epic.

### 4. Cross-Reference and Discover Next Epic

Cross-reference sprint-status.yaml with epics.md to find the next epic to process:

1. List all epics from sprint-status.yaml in epic order (ascending by number)
2. Filter to epics where status != "done"
3. For each candidate epic, verify against epics.md that the epic exists and has stories defined
4. **Self-heal check:** If sprint-status.yaml says an epic is "backlog" but git branches or merged PRs suggest stories have been completed, note the discrepancy in the pipeline state but proceed — the execute-epic sub-workflow will handle the actual state
5. Select the **first** non-done epic (lowest epic number)

### 5. Check Halt Condition

If **no non-done epics remain** after step 4:

- Display: "**All epics complete! Pipeline has no remaining work.**"
- Display cumulative summary from {pipelineStateFile} if it exists (total cycles, completed epics list)
- **HALT** — the pipeline is finished. Do not proceed to any further steps.

### 6. Create/Update Pipeline State

Create or update {pipelineStateFile} with:

```yaml
startedAt: "[current ISO 8601 timestamp, or preserve existing]"
currentEpic: [epic number]
currentEpicName: "[epic name from epics.md]"
currentPhase: "execute"
cycleCount: [increment from previous, or 1 if new]
completedEpics:
  # preserve any existing entries
  - epicNumber: [N]
    epicName: "[name]"
    completedAt: "[timestamp]"
```

Display: "**Pipeline initialized — Epic {currentEpic}: {currentEpicName}. Starting execution phase.**"

### 7. Auto-Proceed to Execute Phase

Display: "**Proceeding to execute Epic {currentEpic}...**"

#### Menu Handling Logic:

- After pipeline state is created/updated, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after initialization

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state checked for resumption correctly
- Sprint status and epics file cross-referenced
- Next non-done epic discovered (or halt condition reached)
- Pipeline state file created/updated with correct epic and phase
- Auto-proceeded to step 02

### ❌ SYSTEM FAILURE:

- Not checking for existing pipeline state before discovering
- Using only one source (sprint-status OR epics) instead of cross-referencing
- Skipping halt condition check when no epics remain
- Halting for user input
- Not creating/updating pipeline state file

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
