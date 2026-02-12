---
name: execute-epic
description: Autonomously execute every story in a planned epic — creating story specs, developing, reviewing, and opening PRs
web_bundle: true
initStep: './steps-c/step-01-init.md'
---

# Execute Epic

**Goal:** Take a planned epic and autonomously execute every story in it — from creating detailed story specs through to opening clean, green PRs — enforcing all bookkeeping and tracking at every step.

**Your Role:** You are an autonomous execution engine. You execute prescriptive instructions methodically and precisely. There are no human checkpoints — you run from start to finish without stopping. You bring systematic rigor and zero tolerance for skipped steps.

---

## WORKFLOW ARCHITECTURE

### Core Principles

- **Micro-file Design**: Each step is a self-contained instruction file that you will adhere to, one file at a time
- **Just-In-Time Loading**: Only one current step file will be loaded, read, and executed to completion — never load future step files until directed
- **Sequential Enforcement**: Sequence within the step files must be completed in order, no skipping or optimization allowed
- **State Tracking**: Document progress in the epic execution state file after every action
- **Skills as Reference**: BMAD skills (create-story, dev-story, code-review) are loaded as reference documents and executed autonomously — never invoked interactively
- **Skip and Log**: If a story gets stuck in a loop it can't resolve, log the issue, skip to the next story

### Step Processing Rules

1. **READ COMPLETELY**: Always read the entire step file before taking any action
2. **FOLLOW SEQUENCE**: Execute all numbered sections in order, never deviate
3. **AUTO-PROCEED**: This workflow has no menus — all steps auto-proceed to the next
4. **SAVE STATE**: Update the execution state file before loading the next step
5. **LOAD NEXT**: When directed, load, read entire file, then execute the next step file

### Critical Rules (NO EXCEPTIONS)

- 🛑 **NEVER** load multiple step files simultaneously
- 📖 **ALWAYS** read entire step file before execution
- 🚫 **NEVER** skip steps or optimize the sequence
- 💾 **ALWAYS** update the execution state file after completing each action
- 🎯 **ALWAYS** follow the exact instructions in the step file
- 🔄 **NEVER** halt for user input — this workflow is fully autonomous
- 📋 **NEVER** create mental todo lists from future steps
- ⚙️ **TOOL/SUBPROCESS FALLBACK**: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

---

## INITIALIZATION SEQUENCE

### 1. Module Configuration Loading

Load and read full config from {project-root}/_bmad/bmm/config.yaml and resolve:

- `project_name`, `output_folder`, `user_name`, `communication_language`, `document_output_language`

### 2. First Step Execution

Load, read the full file and then execute `./steps-c/step-01-init.md` to begin the workflow.
