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
- 🚫 FORBIDDEN to merge PRs in normal mode — the user merges on GitHub
- ✅ In YOLO mode: merge autonomously using merge commits (`gh pr merge --merge`) after all gates pass
- 💬 In normal mode: after addressing findings, always re-present the PR for another review cycle
- 🔄 In normal mode: the [R] handler loops back to the menu — this is the inner review loop
- 🔄 In YOLO mode: the review-address cycle runs autonomously until all gates pass, then auto-merges

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

#### AC Satisfaction Check (before presenting PR)

Before displaying the PR, verify that acceptance criteria are fully satisfied:

1. **Read the story artifact** from `{implementation_artifacts}/{storyKey}.md`
2. **Locate AC status records**, in this order of preference:
   - The `### Completion Notes List` section, looking for lines of the form: `- AC{n} (...): {STATUS}`
   - If not present there, check any Dev Agent Record or later sections that list ACs in the same `AC{n}: STATUS` pattern
3. **Interpret each AC's status deterministically:**
   - Treat `SATISFIED` or `MET` as fully satisfied
   - Treat `PARTIALLY SATISFIED`, `NOT SATISFIED`, `NOT MET`, or any other non-satisfied/ambiguous status as **incomplete**
   - If an AC has **no explicit status line** (no `AC{n} (...): STATUS` entry), treat its status as **unknown** and handle it as incomplete
4. **If any AC is incomplete or has unknown status**, display a warning before the PR summary:

   "**WARNING: Incomplete or Unknown Acceptance Criteria Status Detected**

   The following ACs are not fully satisfied or lack an explicit `SATISFIED`/`MET` status:
   - **AC{n}:** {ac-title} — Status: {status-or-UNKNOWN}
   _(repeat for each such AC)_

   **Project expectation:** ACs should be either fully `SATISFIED`/`MET` or renegotiated with the project lead before final sign-off on the story. Options:
   - Send the PR back for rework to fully satisfy the ACs
   - Renegotiate the ACs with the project lead if the original criteria are infeasible
   - Proceed with review only if the user explicitly acknowledges and accepts the current incomplete/unknown AC state for this iteration"

5. **If and only if all ACs have explicit `SATISFIED` or `MET` statuses:** Proceed silently — no extra output needed

**Normal mode note:** This is an informational workflow flag, not an automatic hard block. The user decides the next action after seeing the warning, but project policy still expects ACs to be fully satisfied or formally renegotiated before final approval/merge.

**YOLO mode note:** The workflow never halts. When incomplete or unknown ACs are detected, the agent MUST exercise judgment:
- **If the AC can reasonably be completed** (e.g., missing test, missing validation, documentation gap): implement the fix, update the story artifact AC status to `SATISFIED`, commit, and push before proceeding to merge.
- **If the AC cannot be completed in this context** (e.g., requires external input, infeasible scope, needs renegotiation): log a warning in the review notes file (`review-notes-{epicId}.md`) under a "Deferred ACs" section explaining why, and proceed to merge with the AC left incomplete.

Display:

"**Reviewing PR #{pr-number}: {title}**

**Story:** {storyId} — {story-title}
**Branch:** {branch} → {baseBranch}
**URL:** {pr-url}
**Changes:** +{additions} / -{deletions} across {changedFiles} files"

### 3. Update State

Update {stateFile}: set current PR status to "reviewing".

### 4. Mode Branch

Read `yoloMode` from {stateFile}.

#### 4a. Normal Mode (yoloMode: false)

Append to the PR presentation:

"**Please review this PR on GitHub, then tell me:**

**[R]** I submitted a review (findings to address)
**[M]** I merged it"

##### EXECUTION RULES:

- ALWAYS halt and wait for user input after presenting the menu
- This is the primary human-in-the-loop checkpoint
- User can chat or ask questions — always respond and redisplay the menu

##### Menu Handling Logic:

- **IF R:** Execute the Review-Address Cycle (section 5 below), then redisplay this menu — re-present the PR for another review round
- **IF M:** Execute the Merge Handler (section 6 below), then route to next step
- **IF Any other:** Help user respond, then redisplay this menu

#### 4b. YOLO Mode (yoloMode: true)

**Do NOT halt for user input. Execute the following autonomously:**

1. Execute the YOLO Review-Address Cycle (section 5-YOLO below) — this handles automated review findings and CI/Cubic gates
2. After all gates pass, execute the YOLO Merge (section 6-YOLO below)
3. Route to next step

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
- **Before exiting:** Verify every Cubic comment has a reply. List all Cubic comments:
  `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
  For each Cubic comment, confirm it has at least one non-Cubic reply (either "Addressed in [commit]" or "Not addressing: [reason]"). If any Cubic comment lacks a reply, reply to it now before proceeding.
- Gate passes. Exit loop.

#### 5e. Report Back

Display:

"**Review findings addressed and pushed. CI/Cubic gate passed.**

Please review the updated PR on GitHub."

**Return to [Menu Options](#4a-normal-mode-yolomode-false)** for the next review round.

---

### 5-YOLO. YOLO Review-Address Cycle (Autonomous — YOLO mode only)

This section runs fully autonomously. Do NOT halt for user input at any point.

#### 5-YOLO-a. Ensure Branch is Rebased

Before reviewing, ensure the branch is up-to-date with its base:
```
git fetch origin {baseBranch}
git rebase origin/{baseBranch}
```

If conflicts occur, resolve them using the same Conflict Resolution Protocol from step-03 (read both sides, reason about correct merged state, resolve with explanation). After resolution:
```
git push --force-with-lease origin {branch-name}
```

If the branch was already up-to-date, no force push needed.

#### 5-YOLO-b. Run CI/Cubic Gate

Execute the same CI/Cubic gate as described in section 5d. This ensures the PR passes all automated quality checks before merge.

- Wait for CI and Cubic
- If any gate produces findings: address them, commit, push, and re-run the gate loop
- Follow the same {maxCiIterations} limit

#### 5-YOLO-c. Address Automated Findings

If Cubic produces review findings:

1. Read and understand each finding
2. Implement fixes
3. Reply to every review comment on the PR (addressed or declined with reason)
4. Commit: `fix: address automated review findings for story {storyId}`
5. Push and re-run the gate loop (back to 5-YOLO-b)

#### 5-YOLO-d. Gate Passed

When CI passes and no Cubic findings remain:

Display: "**All gates passed for PR #{pr-number}. Proceeding to merge...**"

Proceed to section 6-YOLO.

---

### 6-YOLO. YOLO Merge (Autonomous — YOLO mode only)

The agent merges the PR autonomously using a merge commit.

#### 6-YOLO-a. Merge the PR

```
gh pr merge {pr-number} --merge
```

If the merge fails (e.g., branch not up-to-date), rebase and force-push again, re-run the gate, and retry the merge.

#### 6-YOLO-b. Verify Merge

Confirm the PR is merged:
```
gh pr view {pr-number} --json state,mergedAt
```

If not merged, report the error and retry once. If still not merged after retry, log the issue and halt with an error for user intervention (this is the only case where YOLO mode halts).

#### 6-YOLO-c. Post-Merge Handling

Execute the same post-merge steps as the normal Merge Handler (section 6b through 6e):
- Update story artifact status to "done" (6b)
- Update story artifact content if review changed scope (6b2)
- Write review process notes (6c)
- Update review state file (6d)
- Route to next step (6e)

---

### 6. Merge Handler (Normal mode — triggered by [M])

The user has merged this PR on GitHub.

#### 6a. Verify Merge

Confirm the PR is actually merged:
```
gh pr view {pr-number} --json state,mergedAt
```

If not merged, inform the user and return to menu.

#### 6b. Update Story Artifact and Sprint Status

Read the story's artifact file from {implementation_artifacts}/{storyKey}.md.
Update the Status line to "done".

**Update sprint-status.yaml (main-only tracking):**
Since epic-review runs on main after merge, this is the correct place to sync sprint-status:
1. Read the FULL sprint-status.yaml file
2. Find the development_status entry for this story's key
3. Update its value to "done"
4. Save the file, preserving ALL comments and structure including STATUS DEFINITIONS

Commit both changes together:
```
chore: mark story {storyId} done in sprint-status and story artifact
```

#### 6b2. Update Story Artifact Content (if review changed scope)

After updating the status, check whether the PR review process introduced significant scope changes that make the story artifact stale.

1. **Read the story artifact** from `{implementation_artifacts}/{storyKey}.md`
2. **Get the PR diff and summary:**
   ```bash
   # Show a high-level summary of changes
   gh pr diff {pr-number} --stat

   # Show the full diff for detailed comparison
   gh pr diff {pr-number}
   ```
3. **Compare the story artifact against the actual merged changes:**
   - Review the Acceptance Criteria section — were any ACs satisfied via a different approach than the story described?
   - Review the Dev Notes / technical approach — did the implementation deviate from the planned approach?
   - Review the Tasks section — were tasks added, removed, or fundamentally changed during review?
4. **Determine if divergence is significant.** Significant divergence includes:
   - Architecture changes (e.g., different mode, different component, different integration pattern)
   - ACs satisfied via a fundamentally different approach than described
   - New components or files not anticipated in the story
   - Removed or replaced approaches from the original plan
   - Renegotiated acceptance criteria during review
5. **If significant divergence is detected:**
   - Update the story artifact's Acceptance Criteria to reflect the final outcomes
   - Update the Dev Notes / technical approach to describe what was actually implemented
   - Update the Completion Notes to mention the review-driven changes
   - Commit:
     ```
     chore: update story {storyId} artifact to reflect review changes
     ```
6. **If no significant divergence:** Skip — no update needed. The story artifact already accurately describes the shipped state.

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
- **Normal mode:** User signal received (R or M) before taking action
- **YOLO mode:** Automated review, gate enforcement, and merge without halting
- Review findings addressed completely and pushed
- CI/Cubic gate enforced after every push (both modes)
- Story artifact updated to "done" after merge
- State file updated after every action
- Correctly routed to rebase or finalize after merge
- **YOLO mode:** Incomplete ACs either completed or logged with reasoning

### ❌ SYSTEM FAILURE:

- Skipping CI/Cubic gate after pushing changes (either mode)
- **Normal mode:** Merging a PR (user merges, not the agent)
- **YOLO mode:** Halting for user input (except unrecoverable merge failures)
- Not addressing all review findings
- Not updating story artifact after merge
- Not verifying the PR is actually merged before proceeding
- **Normal mode:** Proceeding without user signal

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
