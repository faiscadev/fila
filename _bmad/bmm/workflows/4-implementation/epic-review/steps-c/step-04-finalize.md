---
name: 'step-04-finalize'
description: 'Update sprint-status and epic tracking after all PRs are merged'

stateFile: '{implementation_artifacts}/epic-review-state.yaml'
sprintStatusFile: '{implementation_artifacts}/sprint-status.yaml'
---

# Step 4: Finalize Epic Review

## STEP GOAL:

To update sprint-status.yaml with all stories marked "done" and the epic status set to "done", verify all tracking is consistent, and present a final summary.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file

### Role Reinforcement:

- ✅ You are a disciplined review-cycle facilitator
- ✅ Final tracking must be precise and complete
- ✅ Zero tolerance for inconsistent state

### Step-Specific Rules:

- 🎯 Focus ONLY on tracking updates and final summary
- 🚫 FORBIDDEN to skip any tracking update
- 🚫 FORBIDDEN to leave sprint-status inconsistent with story files

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update sprint-status with all final statuses
- 📖 Verify consistency between sprint-status and story artifact files
- 🚫 FORBIDDEN to skip verification

## CONTEXT BOUNDARIES:

- Available: Review state file, sprint-status, story artifact files
- Focus: Tracking updates and verification
- Limits: All PRs must be merged before this step runs
- Dependencies: step-02 must have processed all PRs as merged

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load Final State

Read {stateFile} to get the complete PR chain. Verify ALL PRs have status "merged".

If any PR is NOT merged, this step was reached in error. Report the issue and halt.

### 2. Update Sprint Status

Read {sprintStatusFile}. For each story in the PR chain:

- Set the story's status to "done"

Set the epic's status to "done".

**Before writing, reason about the correct state:** Read the current sprint-status file completely. Understand what each field currently says. Only change the fields that need updating (the stories in this epic and the epic itself). Do not modify unrelated entries.

Commit:
```
chore: mark epic {epicNumber} complete, update sprint status
```

### 3. Verify Story Artifact Consistency

For each story in the PR chain, read its artifact file from the implementation artifacts folder and verify:

- Status is "done"
- If any story artifact still shows a status other than "done", update it and commit

### 4. Update State File

Update {stateFile}:
- Set all PRs to final "merged" status (should already be)
- Add completedAt timestamp

### 5. Commit Review State File

Commit the updated {stateFile} so the review history is preserved in the repo:
```
git add {stateFile}
git commit -m "chore: update epic-review state for epic {epicNumber} completion"
```

### 6. Checkout Main Branch

Switch back to main and pull the latest (all PRs are now merged):
```
git checkout main
git pull origin main
```

### 7. Present Final Summary

Display:

"**Epic Review Complete: {epic-name}**

All PRs merged and tracking updated.

| # | Story | PR | Status |
|---|-------|----|--------|
| 1 | {title} | #{pr} | merged |
| 2 | {title} | #{pr} | merged |
| ... | | | |

**Sprint Status:** Epic {epicNumber} → done
**Story Files:** All updated to done
**Current Branch:** main (up to date)

Epic review workflow complete."

### 8. No Next Step

This is the final step. The workflow is complete.

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- All PRs verified as merged
- Sprint-status updated: all stories "done", epic "done"
- All story artifact files verified as "done"
- State file updated with completion timestamp
- Review state file committed to repo
- Checked out main branch with latest
- Clear final summary presented

### ❌ SYSTEM FAILURE:

- Sprint-status not updated
- Story artifacts inconsistent with sprint-status
- Epic status not set to "done"
- Not verifying all PRs are actually merged
- Review state file left uncommitted
- Leaving agent on a feature branch instead of main

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
