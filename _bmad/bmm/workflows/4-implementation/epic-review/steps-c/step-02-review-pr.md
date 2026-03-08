---
name: 'step-02-review-pr'
description: 'Present current PR to user, handle review findings or merge signal, enforce CI/Cubic gates'

stateFile: '{implementation_artifacts}/epic-review-state.yaml'
rebaseStepFile: './step-03-rebase-next.md'
finalizeStepFile: './step-04-finalize.md'
maxCiIterations: 5
---

# Step 2: PR Review Loop

## STEP GOAL:

To present the current PR to the user, wait for their signal (submitted a review or merged), and handle each case — addressing review findings autonomously with CI/Cubic gates, or routing to the rebase/finalize step after merge.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are a disciplined review-cycle facilitator
- ✅ You halt ONLY to present PRs and receive user signals
- ✅ Everything between user signals is autonomous
- ✅ CI and Cubic gates are non-negotiable — never skip them

### Step-Specific Rules:

- 🎯 Present the PR clearly with link and summary
- 🚫 FORBIDDEN to skip CI/Cubic checks after pushing changes
- 🚫 FORBIDDEN to merge PRs — the user merges on GitHub
- 💬 After addressing findings, always re-present the PR for another review cycle
- 🔄 The [R] handler loops back to the menu — this is the inner review loop

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file after every significant action
- 📖 Enforce CI/Cubic gate on every push
- 🚫 FORBIDDEN to exceed {maxCiIterations} CI/review iterations per review cycle

## CONTEXT BOUNDARIES:

- Available: Review state file, GitHub PR data, story artifact files
- Focus: PR presentation, review finding resolution, CI/Cubic enforcement
- Limits: Do not merge, do not rebase — those are other steps
- Dependencies: step-01 or step-03 must have set the current PR

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load Current PR

Read {stateFile} to get the current PR index and PR details.

Check out the current PR's branch:
```
git checkout {branch-name}
git pull origin {branch-name}
```

### 2. Present PR to User

Fetch PR details from GitHub:
```
gh pr view {pr-number} --json title,body,url,additions,deletions,changedFiles
```

Display:

"**Reviewing PR #{pr-number}: {title}**

**Story:** {storyId} — {story-title}
**Branch:** {branch} → {baseBranch}
**URL:** {pr-url}
**Changes:** +{additions} / -{deletions} across {changedFiles} files

**Please review this PR on GitHub, then tell me:**

**[R]** I submitted a review (findings to address)
**[M]** I merged it"

### 3. Update State

Update {stateFile}: set current PR status to "reviewing".

### 4. Present MENU OPTIONS

Display: **[R] Submitted a review | [M] Merged**

#### EXECUTION RULES:

- ALWAYS halt and wait for user input after presenting the menu
- This is the primary human-in-the-loop checkpoint
- User can chat or ask questions — always respond and redisplay the menu

#### Menu Handling Logic:

- **IF R:** Execute the Review-Address Cycle (section 5 below), then [Redisplay Menu Options](#4-present-menu-options) — re-present the PR for another review round
- **IF M:** Execute the Merge Handler (section 6 below), then route to next step
- **IF Any other:** Help user respond, then [Redisplay Menu Options](#4-present-menu-options)

---

### 5. Review-Address Cycle (Autonomous — triggered by [R])

This section runs autonomously after the user selects [R]. Do NOT halt for input during this cycle.

#### 5a. Read Review Findings

Fetch all review comments and findings from the PR:
```
gh api repos/{owner}/{repo}/pulls/{pr-number}/reviews
gh api repos/{owner}/{repo}/pulls/{pr-number}/comments
gh pr view {pr-number} --json reviews,comments
```

Read and understand each finding. Group by file.

#### 5b. Address Each Finding

For each review finding:

1. Read the relevant code and understand the context
2. Determine the appropriate fix
3. Implement the fix
4. If the finding requires a regression test, add one

Commit all changes with a descriptive message:
```
fix: address code review findings for story {storyId}
```

#### 5c. Push Changes

```
git push origin {branch-name}
```

#### 5d. CI/Cubic Gate

**CRITICAL: This gate is non-negotiable. Follow every sub-step.**

Set iteration counter to 0.

**GATE LOOP START:**

Increment iteration counter. **If counter > {maxCiIterations}:** Report "CI/Cubic gate could not be resolved after {maxCiIterations} iterations. Presenting PR for user guidance." Break out of gate loop and return to menu.

**Wait for CI checks to complete:**
```
gh pr checks {pr-number} --watch
```

**Check Cubic automated review output:**
After CI checks pass, examine the Cubic check output. It shows:
"AI review completed with N review(s). X issues found across Y files."

- **If 0 issues found:** Cubic has nothing to report. Gate passes. Exit loop.
- **If >0 issues found:** Cubic WILL post a review after a delay. Poll for it:
  ```
  gh api repos/{owner}/{repo}/pulls/{pr-number}/reviews
  ```
  Check every 15-30 seconds, up to 2 minutes, until Cubic's review appears.

**If CI fails OR Cubic has new findings:**
1. Read the failure details or Cubic findings
2. Reply to EVERY new Cubic review comment on the PR BEFORE pushing fixes:
   - For addressed comments: reply with "Addressed in [commit hash]"
   - For declined comments: reply with "Not addressing: [reason]"
   - List Cubic inline comments: `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
   - Reply to each: `gh api repos/{owner}/{repo}/pulls/comments/{comment_id}/replies -f body="..."`
3. Fix the issues in code
4. Add regression tests for Cubic findings where appropriate
5. Commit: `fix: address CI/Cubic findings for story {storyId}`
6. Push: `git push origin {branch-name}`
7. **Go back to GATE LOOP START**

**If CI passes AND no Cubic findings (or 0 issues):**
- Gate passes. Exit loop.

#### 5e. Report Back

Display:

"**Review findings addressed and pushed. CI/Cubic gate passed.**

Please review the updated PR on GitHub."

**Return to [Menu Options](#4-present-menu-options)** for the next review round.

---

### 6. Merge Handler (triggered by [M])

The user has merged this PR on GitHub.

#### 6a. Verify Merge

Confirm the PR is actually merged:
```
gh pr view {pr-number} --json state,mergedAt
```

If not merged, inform the user and return to menu.

#### 6b. Update Story Artifact

Read the story's artifact file from {implementation_artifacts}/{storyKey}.md.
Update the Status line to "done".
Commit:
```
fix: update story {storyId} status to done
```

#### 6c. Write Review Process Notes

**MANDATORY:** After each PR merge, capture process insights for the retrospective.

Append to `{implementation_artifacts}/review-notes-{epicId}.md` (create if it doesn't exist):

```markdown
## PR #{pr-number}: {story-title}

### Gaps in Dev Process
- [Things the dev agent should have caught during development]

### Incorrect Decisions During Development
- [Wrong approach, missing requirements, etc.]

### Deferred Work
- [Work deferred during review, with rationale]

### Patterns for Future Stories
- [Patterns that should inform future stories]
```

This file becomes input for the retrospective workflow, reducing the retro facilitator's reconstruction work.

#### 6d. Update Review State

Update {stateFile}:
- Set current PR status to "merged"
- Increment currentPrIndex

#### 6e. Route to Next Step

Check the PR chain:

- **If there are more PRs remaining:** Display "**PR #{pr-number} merged. Proceeding to rebase next PR...**" Then load, read entirely, then execute {rebaseStepFile}.
- **If this was the last PR:** Display "**All PRs merged! Proceeding to finalization...**" Then load, read entirely, then execute {finalizeStepFile}.

---

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- PR presented clearly with link and summary
- User signal received (R or M) before taking action
- Review findings addressed completely and pushed
- CI/Cubic gate enforced after every push
- Story artifact updated to "done" after merge
- State file updated after every action
- Correctly routed to rebase or finalize after merge

### ❌ SYSTEM FAILURE:

- Skipping CI/Cubic gate after pushing changes
- Merging a PR (user merges, not the agent)
- Not addressing all review findings
- Not updating story artifact after merge
- Not verifying the PR is actually merged before proceeding
- Proceeding without user signal

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
