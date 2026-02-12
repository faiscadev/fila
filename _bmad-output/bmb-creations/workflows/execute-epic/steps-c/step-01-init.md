---
name: 'step-01-init'
description: 'Parse epic plan, build story execution order, initialize or resume state'

nextStepFile: './step-02-story-setup.md'
continueFile: './step-01b-continue.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
---

# Step 1: Initialize Epic Execution

## STEP GOAL:

To parse the epic plan file, analyze story dependencies, build a topological execution order, and initialize the execution state file. If a previous execution exists, route to continuation.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are an autonomous execution engine
- ✅ You execute prescriptive instructions methodically and precisely
- ✅ There are no human checkpoints — you run from start to finish
- ✅ Zero tolerance for skipped steps or incomplete tracking

### Step-Specific Rules:

- 🎯 Focus ONLY on parsing the epic and building the execution plan
- 🚫 FORBIDDEN to start executing stories in this step
- 🚫 FORBIDDEN to halt for user input
- 💬 Auto-proceed once initialization is complete

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Create the execution state file with all stories
- 📖 Track initialization in the state file
- 🚫 FORBIDDEN to skip dependency analysis

## CONTEXT BOUNDARIES:

- Available: BMM config, epic plan file, sprint status file
- Focus: Parsing, dependency analysis, state initialization
- Limits: Do not begin story execution
- Dependencies: None — this is the first step

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Check for Existing State

Look for an existing execution state file at {stateFile}.

- **If the file exists and has stories with status other than "pending":** A previous execution was interrupted. Load, read entirely, then execute {continueFile} to resume.
- **If the file does not exist or all stories are "pending":** Continue to step 2.

### 2. Locate Epic Plan

Find the epic plan file in the project. Look for epic plan files in `_bmad-output/` and `_bmad/bmm/` directories. The epic plan contains story definitions, dependencies, and ordering information.

If no epic plan is found, report the error and halt.

### 3. Parse Stories and Dependencies

From the epic plan file, extract for each story:

- Story ID (e.g., "2.1", "2.2")
- Story title and description
- Dependencies on other stories (which stories must complete first)
- Any relevant dev notes or context

### 4. Build Execution Order

Perform topological sort based on story dependencies:

- Stories with no dependencies come first
- Dependent stories are ordered after their prerequisites
- If multiple stories have no remaining dependencies, maintain their original order from the epic plan

### 5. Determine Base Branch

Use `main` as the base branch unless the epic plan specifies otherwise.

### 6. Create Execution State File

Create {stateFile} with the following structure:

```yaml
epic: [epic name from plan]
baseBranch: main
startedAt: [current date/time]
stories:
  - id: "[story id]"
    title: "[story title]"
    status: pending
    currentPhase: ""
    branch: ""
    pr: null
    dependsOn: []
  # ... repeat for all stories in execution order
skippedIssues: []
```

### 7. Auto-Proceed to Story Execution

Display: "**Epic initialized with [N] stories. Proceeding to first story...**"

#### Menu Handling Logic:

- After state file is created, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after initialization

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Epic plan found and parsed correctly
- All stories extracted with dependencies
- Execution order determined via topological sort
- State file created with all stories as "pending"
- Auto-proceeded to step 02

### ❌ SYSTEM FAILURE:

- Epic plan not found
- Dependencies not analyzed (just using file order)
- State file not created
- Halting for user input
- Skipping dependency analysis

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
