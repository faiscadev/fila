---
name: 'step-04-retro-and-correct'
description: 'Run retrospective then correct-course for the current epic'

nextStepFile: './step-05-cycle-complete.md'
pipelineStateFile: '{output_folder}/pipeline-state.yaml'
retrospectiveWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/retrospective/workflow.yaml'
correctCourseWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/correct-course/workflow.yaml'
retroDocPattern: '{implementation_artifacts}/epic-{epicNumber}-retro*.md'
---

# Step 4: Retrospective & Correct-Course

## STEP GOAL:

To run the retrospective sub-workflow for the current epic (generating a retro document), then run the correct-course sub-workflow (applying retro learnings to planning artifacts). This step handles both phases and supports resumption mid-step — if `currentPhase` is already "correct-course", it skips the retrospective and goes straight to correct-course.

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

- 🎯 This step has TWO sub-phases: retrospective, then correct-course
- 🚫 FORBIDDEN to skip either sub-phase (unless resuming past retro)
- 🚫 FORBIDDEN to halt for user input
- 🚫 FORBIDDEN to let sub-workflows auto-discover the epic
- 💬 Pass retro doc path explicitly to correct-course

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update pipeline-state.yaml between sub-phases and after completion
- 📖 Locate the retro doc by pattern matching after retrospective runs
- 🚫 FORBIDDEN to run correct-course without a retro doc path

## CONTEXT BOUNDARIES:

- Available: pipeline-state.yaml with currentEpic, currentPhase ("retro" or "correct-course")
- Focus: Generate retro doc, then apply learnings via correct-course
- Limits: Do not finalize the cycle — that's step 05
- Dependencies: step-03-review must have completed (epic PRs merged)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Pipeline State

Load {pipelineStateFile} and extract:

- `currentEpic` — the epic number
- `currentEpicName` — the epic name
- `currentPhase` — either "retro" or "correct-course"

### 2. Run Retrospective (If Phase is "retro")

**Skip this section if `currentPhase` is "correct-course"** — the retrospective already ran in a previous session.

Display: "**Running retrospective for Epic {currentEpic}: {currentEpicName}**"

Load and execute the retrospective workflow at {retrospectiveWorkflow}.

**Explicit parameters to pass:**

- **Epic:** `epic {currentEpic}` — the retro workflow must know which epic to retrospect
- **Mode:** YOLO — run autonomously, no human gates
- **Instruction:** "Run retrospective for epic {currentEpic}: {currentEpicName}. Analyze what went well, what didn't, and extract actionable lessons. Run autonomously without stopping for human input."

**On completion:** A retro document should exist at a path matching `{retroDocPattern}`.

Update {pipelineStateFile}: set `currentPhase` to `"correct-course"`

### 3. Locate Retro Document

Search for the retro document using pattern: `{retroDocPattern}`

- Replace `{epicNumber}` with the actual current epic number
- If multiple matches, use the most recently modified file
- If no match found, search more broadly in `{implementation_artifacts}` for any file containing "retro" and the epic number

**Store the retro doc path** — it must be passed explicitly to correct-course.

Display: "**Found retro doc: {retro_doc_path}**"

### 4. Run Correct-Course

Display: "**Running correct-course for Epic {currentEpic} with retro: {retro_doc_path}**"

Load and execute the correct-course workflow at {correctCourseWorkflow}.

**Explicit parameters to pass:**

- **Epic:** `epic {currentEpic}` — the correct-course workflow must know which epic's learnings to apply
- **Retro doc:** the exact file path found in step 3
- **Mode:** YOLO — apply changes autonomously, no human gates
- **Instruction:** "Apply learnings from retrospective for epic {currentEpic}: {currentEpicName}. Retro document is at: {retro_doc_path}. Update planning artifacts (epics, stories, architecture) based on retro findings. Run autonomously without stopping for human input."

**CRITICAL:** The correct-course workflow MUST receive both the epic number AND the retro doc path explicitly. Without the retro doc path, it cannot know what learnings to apply.

**On completion:** Planning artifacts should be updated with retro learnings.

### 5. Update Pipeline State

Update {pipelineStateFile}:

- Set `currentPhase` to `"complete"`

Display: "**Epic {currentEpic} retro and correct-course complete. Advancing to cycle completion.**"

### 6. Auto-Proceed to Cycle Complete

Display: "**Proceeding to finalize cycle for Epic {currentEpic}...**"

#### Menu Handling Logic:

- After pipeline state is updated, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after both sub-phases complete

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Pipeline state read correctly, phase routing handled (retro vs correct-course)
- Retrospective sub-workflow invoked with explicit epic number (unless skipped for resumption)
- Retro document located by pattern matching
- Correct-course sub-workflow invoked with explicit epic number AND retro doc path
- Pipeline state updated between sub-phases and to currentPhase="complete"
- Auto-proceeded to step 05

### ❌ SYSTEM FAILURE:

- Running correct-course without locating the retro doc first
- Not passing retro doc path explicitly to correct-course
- Letting sub-workflows auto-discover the epic
- Running retrospective when phase is already "correct-course" (wastes a retro)
- Not updating pipeline state between sub-phases
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
