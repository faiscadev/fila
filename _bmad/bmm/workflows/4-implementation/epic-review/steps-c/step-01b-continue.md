---
name: 'step-01b-continue'
description: 'Resume an interrupted epic review from where it left off'

stateFile: '{implementation_artifacts}/epic-review-state.yaml'
reviewStep: './step-02-review-pr.md'
rebaseStep: './step-03-rebase-next.md'
finalizeStep: './step-04-finalize.md'
---

# Step 1b: Continue Epic Review

## STEP GOAL:

To resume an interrupted epic review session by reading the existing state file, determining where the review left off, and routing to the correct step.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file

### Role Reinforcement:

- ✅ You are a disciplined review-cycle facilitator
- ✅ You resume seamlessly from saved state

### Step-Specific Rules:

- 🎯 Focus ONLY on reading state and routing to the correct step
- 🚫 FORBIDDEN to start reviewing or rebasing in this step

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 📖 Read the entire state file to understand current progress

## CONTEXT BOUNDARIES:

- Available: Existing review state file from a previous session
- Focus: State recovery and routing
- Limits: Do not perform any review or rebase actions
- Dependencies: State file must exist (checked by step-01)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Review State

Load {stateFile} completely. Extract:

- Epic name and number
- Current PR index
- The full PR chain with per-PR statuses
- Which PRs are merged, which are pending

### 2. Present Resume Summary

"**Welcome back! Resuming epic review for: {epic-name}**

Progress so far:

| # | Story | PR | Status |
|---|-------|----|--------|
| 1 | {title} | #{pr} | {status} |
| 2 | {title} | #{pr} | {status} |
| ... | | | |

**Resuming from PR #{current-pr-number} ({current-story-title})...**"

### 3. Route to Correct Step

Determine the next action based on state:

- **If the current PR's status is "pending" or "reviewing":** The user needs to review this PR next. Load, read entirely, then execute {reviewStep}.
- **If the current PR's status is "merged" and there are more PRs:** The next PR needs rebasing. Load, read entirely, then execute {rebaseStep}.
- **If all PRs are "merged":** Proceed to finalization. Load, read entirely, then execute {finalizeStep}.

#### EXECUTION RULES:

- This is an auto-proceed step — route immediately after presenting the summary
- Do not halt for user input

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- State file read completely
- Resume summary presented clearly
- Routed to correct step based on state
- User knows where they left off

### ❌ SYSTEM FAILURE:

- State file not read
- Routed to wrong step
- Starting review or rebase actions in this step

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
