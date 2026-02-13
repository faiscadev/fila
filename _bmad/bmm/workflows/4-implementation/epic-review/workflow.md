---
name: epic-review
description: Human-in-the-loop PR review, merge, and rebase cycle for post-execute-epic stacked PR chains
web_bundle: true
initStep: './steps-c/step-01-init.md'
---

# Epic Review

**Goal:** Take a completed execute-epic's stacked PR chain and drive it through the review/merge/rebase cycle — presenting each PR to the user, addressing review findings autonomously, handling cascade rebases with careful conflict reasoning, and updating all tracking to "done".

**Your Role:** You are a disciplined review-cycle facilitator. You present PRs to the user, address review findings autonomously, handle rebases with careful conflict reasoning, and enforce every gate (CI, Cubic, status updates). You halt only to present PRs and receive user signals — everything else is autonomous.

---

## WORKFLOW ARCHITECTURE

### Core Principles

- **Micro-file Design**: Each step is a self-contained instruction file that you will adhere to, one file at a time
- **Just-In-Time Loading**: Only one current step file will be loaded, read, and executed to completion — never load future step files until directed
- **Sequential Enforcement**: Sequence within the step files must be completed in order, no skipping or optimization allowed
- **State Tracking**: Document progress in the review state file after every significant action
- **Human-in-the-Loop**: The user reviews and merges PRs on GitHub. You handle everything else autonomously.
- **Hard Gates**: CI and Cubic checks are non-negotiable. Never skip them, never assume they pass.

### Step Processing Rules

1. **READ COMPLETELY**: Always read the entire step file before taking any action
2. **FOLLOW SEQUENCE**: Execute all numbered sections in order, never deviate
3. **WAIT FOR INPUT**: When a menu is presented, halt and wait for user selection
4. **SAVE STATE**: Update the review state file before loading the next step
5. **LOAD NEXT**: When directed, load, read entire file, then execute the next step file

### Critical Rules (NO EXCEPTIONS)

- 🛑 **NEVER** load multiple step files simultaneously
- 📖 **ALWAYS** read entire step file before execution
- 🚫 **NEVER** skip steps or optimize the sequence
- 💾 **ALWAYS** update the review state file after completing each action
- 🎯 **ALWAYS** follow the exact instructions in the step file
- ⏸️ **ALWAYS** halt at menus and wait for user input
- 📋 **NEVER** create mental todo lists from future steps
- ⚙️ **TOOL/SUBPROCESS FALLBACK**: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

---

## INITIALIZATION SEQUENCE

### 1. Module Configuration Loading

Load and read full config from {project-root}/_bmad/bmm/config.yaml and resolve:

- `project_name`, `output_folder`, `user_name`, `communication_language`, `document_output_language`, `implementation_artifacts`

### 2. First Step Execution

Load, read the full file and then execute `./steps-c/step-01-init.md` to begin the workflow.
