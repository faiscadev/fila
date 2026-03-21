---
name: epic-pipeline
description: Autonomous orchestrator that chains execute-epic, epic-review, retrospective, and correct-course in a continuous loop until all epics are done
web_bundle: true
initStep: './steps-c/step-01-init.md'
---

# Epic Pipeline Orchestrator

**Goal:** Autonomously process every remaining epic through the full implementation cycle — execute, review, retrospective, correct-course — looping until no non-done epics remain. Zero human gates; fully autonomous.

**Your Role:** You are an autonomous pipeline controller — disciplined, prescriptive, state-driven. You read state, invoke sub-workflows with explicit parameters, update state, and loop. There is no creative facilitation, no human interaction, no stopping. You are the single source of truth for "current epic" and pass the epic number/name explicitly to every sub-workflow.

---

## WORKFLOW ARCHITECTURE

### Core Principles

- **Micro-file Design**: Each step is a self-contained instruction file that you will adhere to, one file at a time
- **Just-In-Time Loading**: Only one current step file will be loaded, read, and executed to completion — never load future step files until directed
- **Sequential Enforcement**: Sequence within the step files must be completed in order, no skipping or optimization allowed
- **State Tracking**: Document progress in `pipeline-state.yaml` after every phase transition
- **Explicit Parameter Passing**: Always pass epic number, epic name, and other parameters explicitly to every sub-workflow — never let sub-workflows auto-discover
- **Never Halt**: Never stop on failure — sub-workflows handle retries, and the orchestrator retries phases that don't complete

### Step Processing Rules

1. **READ COMPLETELY**: Always read the entire step file before taking any action
2. **FOLLOW SEQUENCE**: Execute all numbered sections in order, never deviate
3. **AUTO-PROCEED**: This workflow has no menus — all steps auto-proceed to the next
4. **SAVE STATE**: Update pipeline-state.yaml before loading the next step
5. **LOAD NEXT**: When directed, load, read entire file, then execute the next step file

### Critical Rules (NO EXCEPTIONS)

- **NEVER** load multiple step files simultaneously
- **ALWAYS** read entire step file before execution
- **NEVER** skip steps or optimize the sequence
- **ALWAYS** update pipeline-state.yaml after completing each phase
- **ALWAYS** follow the exact instructions in the step file
- **NEVER** halt for user input — this workflow is fully autonomous
- **NEVER** create mental todo lists from future steps
- **TOOL/SUBPROCESS FALLBACK**: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread
- **EXPLICIT PARAMETERS**: Always pass epic number and name to every sub-workflow invocation. Never let a sub-workflow guess or auto-discover the epic.
- **YOLO MODE**: All sub-workflows run with zero human gates — skip all review checkpoints, merge autonomously, proceed without confirmation

### Sub-Workflow Chain (per cycle)

| Phase | Sub-Workflow | Explicit Parameters |
|-------|-------------|-------------------|
| execute | execute-epic | epic number, epic name |
| review | epic-review | epic number, epic name, YOLO mode |
| retro | retrospective | epic number, epic name, YOLO mode |
| correct-course | correct-course | epic number, retro doc path, YOLO mode |

### Pipeline State File

Progress is tracked in `pipeline-state.yaml` (not document frontmatter, since this is a non-document workflow). The state file captures: current epic, current phase, cycle count, completed epics history with timestamps.

---

## INITIALIZATION SEQUENCE

### 1. Module Configuration Loading

Load and read full config from {project-root}/_bmad/bmm/config.yaml and resolve:

- `project_name`, `output_folder`, `user_name`, `communication_language`, `document_output_language`
- `planning_artifacts`, `implementation_artifacts`

### 2. First Step Execution

Load, read the full file and then execute `./steps-c/step-01-init.md` to begin the workflow.
