---
name: 'step-06-story-complete'
description: 'Mark story complete and loop back to next story or proceed to epic completion'

storySetupStep: './step-02-story-setup.md'
epicCompleteStep: './step-07-epic-complete.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
---

# Step 6: Story Completion & Loop

## STEP GOAL:

To mark the current story as completed (or skipped) in the state file and determine whether to loop back for the next story or proceed to epic completion.

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

- 🎯 Focus ONLY on state transition and loop control
- 🚫 FORBIDDEN to halt for user input
- 💬 This is a routing step — mark complete and route to the next action

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file with story completion
- 📖 Determine next action based on remaining stories
- 🚫 FORBIDDEN to skip state update

## CONTEXT BOUNDARIES:

- Available: Execution state file
- Focus: State transition and routing
- Limits: No new work — just routing
- Dependencies: step-05 (or step-04 if skipped) must have completed

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Mark Current Story Complete

Load {stateFile} and update the current story:

- **If the story was not skipped:** Set status to "completed"
- **If the story was skipped:** Status is already "skipped" (set by step-04 or step-05)

Clear `currentPhase` (set to empty string).
Mark the story's status as `completed` in the state file.

### 1b. Sync Story Artifact Status to "review"

Update the story artifact file's `Status:` line to "review".

Load the story artifact file (in `_bmad-output/implementation-artifacts/`). Change the `Status:` line to `Status: review` (regardless of its current value). Save but do NOT commit yet — commit all tracking changes together in step 1c.

**Note:** Sprint-status.yaml is NOT updated on feature branches. The epic-review workflow's finalize step handles all sprint-status updates on main after PR merge.

### 1c. Commit and Push Tracking Files

**CRITICAL:** Before switching to a new branch for the next story, commit and push tracking file changes (state file, story artifact) on the current branch in a single commit. The state file reverts to main's version when switching branches — if not committed first, progress is lost.

**Do NOT include sprint-status.yaml** — it is only updated on main.

### 2. Check for Remaining Stories

Count stories with status "pending":

- **If pending stories remain:** Proceed to step 3
- **If no pending stories:** Proceed to step 4

### 3. Loop Back to Next Story

Display: "**Story {story-id} complete. Moving to next story ({next-story-id}: {next-story-title})...**"

Set the next pending story as current in {stateFile}.

Load, read entirely, then execute {storySetupStep} to begin the next story.

### 4. All Stories Done — Proceed to Epic Completion

Display: "**All stories processed. Proceeding to epic completion...**"

Load, read entirely, then execute {epicCompleteStep}.

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Route to either step-02 (next story) or step-07 (epic complete)

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Current story marked completed or skipped in state file
- Remaining stories correctly counted
- Routed to correct next action (next story or epic complete)
- No halting for user input

### ❌ SYSTEM FAILURE:

- Story not marked in state file
- Routing to wrong step
- Re-executing completed stories
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
