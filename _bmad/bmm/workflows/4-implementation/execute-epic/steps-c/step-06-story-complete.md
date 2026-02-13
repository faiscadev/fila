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

Clear currentPhase (set to empty string).

### 1b. Update Sprint Status to "done"

**CRITICAL:** Update sprint-status.yaml to reflect story completion. This is the step that keeps sprint-status in sync — skipping it causes stale tracking.

Load the FULL sprint-status.yaml file. Find the development_status entry for the current story key. Update its value to "done". Save the file, preserving ALL comments and structure including STATUS DEFINITIONS. Commit the change.

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
