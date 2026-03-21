---
name: 'step-02-execute'
description: 'Invoke execute-epic sub-workflow for the current epic'

nextStepFile: './step-03-review.md'
pipelineStateFile: '{output_folder}/pipeline-state.yaml'
executeEpicWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/execute-epic/workflow.yaml'
---

# Step 2: Execute Epic

## STEP GOAL:

To invoke the execute-epic sub-workflow for the current epic, passing the epic number and name explicitly. Upon completion, update pipeline state to the "review" phase and proceed.

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

- 🎯 Focus ONLY on invoking execute-epic and updating pipeline state
- 🚫 FORBIDDEN to skip the sub-workflow invocation
- 🚫 FORBIDDEN to halt for user input
- 🚫 FORBIDDEN to let the sub-workflow auto-discover the epic — you MUST pass it explicitly
- 💬 Auto-proceed after execution completes

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update pipeline-state.yaml after sub-workflow completes
- 📖 Pass ALL parameters explicitly to the sub-workflow
- 🚫 FORBIDDEN to modify sprint-status.yaml directly — the sub-workflow handles that

## CONTEXT BOUNDARIES:

- Available: pipeline-state.yaml with currentEpic and currentEpicName
- Focus: Invoke execute-epic, then advance phase
- Limits: Do not review or merge PRs — that's the next phase
- Dependencies: step-01-init must have set currentEpic and currentPhase="execute"

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Pipeline State

Load {pipelineStateFile} and extract:

- `currentEpic` — the epic number to execute
- `currentEpicName` — the epic name for display and parameter passing

Display: "**Executing Epic {currentEpic}: {currentEpicName}**"

### 2. Invoke Execute-Epic Sub-Workflow

Load and execute the execute-epic workflow at {executeEpicWorkflow}.

**Explicit parameters to pass:**

- **Epic:** `epic {currentEpic}` — the execute-epic workflow must know which epic to process
- **Mode:** Fully autonomous, YOLO — skip all human gates, no stopping for review
- **Instruction:** "Execute epic {currentEpic}: {currentEpicName}. Create story specs, develop all stories, open PRs. Do not stop for human review. Run autonomously from start to finish."

**CRITICAL:** The execute-epic workflow MUST receive the epic number explicitly. Do NOT let it auto-discover or pick an epic on its own.

**On completion:** The execute-epic workflow will have created story branches, opened PRs, and updated the execution state file. All stories should be in "review" status with open PRs.

**On failure/incomplete:** If the sub-workflow exits without completing all stories, note the issue but proceed anyway — the review phase will handle what's available, and incomplete stories can be retried in a future cycle.

### 3. Update Pipeline State

Update {pipelineStateFile}:

- Set `currentPhase` to `"review"`

Display: "**Epic {currentEpic} execution complete. Advancing to review phase.**"

### 4. Auto-Proceed to Review Phase

Display: "**Proceeding to review Epic {currentEpic}...**"

#### Menu Handling Logic:

- After pipeline state is updated, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after execution completes

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state read correctly
- Execute-epic sub-workflow invoked with explicit epic number and name
- Sub-workflow ran in YOLO mode (no human gates)
- Pipeline state updated to currentPhase="review"
- Auto-proceeded to step 03

### ❌ SYSTEM FAILURE:

- Not reading pipeline state before invoking sub-workflow
- Letting execute-epic auto-discover the epic instead of passing it explicitly
- Not updating pipeline state after completion
- Halting for user input
- Skipping the sub-workflow invocation

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
