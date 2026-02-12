---
name: 'step-07-epic-complete'
description: 'Final verification and summary of the epic execution'

stateFile: '{output_folder}/epic-execution-state.yaml'
---

# Step 7: Epic Complete

## STEP GOAL:

To perform final verification that all PRs are open and green, and present a complete summary of the epic execution including completed stories, skipped stories, and PR links.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are an autonomous execution engine
- ✅ You execute prescriptive instructions methodically and precisely
- ✅ This is the final step — present clear results

### Step-Specific Rules:

- 🎯 Focus ONLY on verification and summary
- 🚫 FORBIDDEN to make code changes
- 💬 Present a clear, comprehensive summary

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file with completion timestamp
- 📖 Verify all PRs via GitHub CLI

## CONTEXT BOUNDARIES:

- Available: Execution state file, GitHub PRs
- Focus: Verification and reporting
- Limits: Do not merge PRs — leave for human review
- Dependencies: All stories must have been processed (completed or skipped)

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load Final State

Load {stateFile} and compile:
- Total stories
- Completed stories (with PR numbers)
- Skipped stories (with reasons)

### 2. Verify PRs via GitHub

For each completed story with a PR number:
```
gh pr view {pr-number} --json state,statusCheckRollup,reviews,url
```

Verify:
- PR is open (not merged, not closed)
- CI checks are passing
- Note any that have degraded since opening

### 3. Update State with Completion

Update {stateFile}:
- Add `completedAt: [current date/time]`
- Add `summary` section with final counts

### 4. Present Epic Execution Summary

Display a comprehensive summary:

```
**EPIC EXECUTION COMPLETE**

**Epic:** {epic name}
**Started:** {startedAt}
**Completed:** {completedAt}

**Results:**
- Completed: {N}/{total} stories
- Skipped: {N}/{total} stories

**Completed Stories:**
| Story | Title | Branch | PR | Status |
|-------|-------|--------|------|--------|
| {id}  | {title} | {branch} | #{pr} | ✅ Green |
...

**Skipped Stories:**
| Story | Phase | Reason |
|-------|-------|--------|
| {id}  | {phase} | {reason} |
...

**All PRs are open and ready for human review.**
```

### 5. Workflow Complete

This is the final step. No next step to load.

The workflow is complete. All PRs are open for Lucas to review.

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- All PRs verified as open and green via GitHub CLI
- Comprehensive summary presented with all stories, branches, PRs
- Skipped stories clearly documented with reasons
- State file updated with completion timestamp
- PRs left open for human review

### ❌ SYSTEM FAILURE:

- PRs not verified via GitHub
- Summary missing stories or PR links
- Skipped stories not documented
- PRs merged instead of left open

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
