---
name: 'step-05-pr-ci'
description: 'Update tracking, open PR, iterate on CI and automated PR review feedback until green'

nextStepFile: './step-06-story-complete.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
sprintStatusFile: '{output_folder}/sprint-status.yaml'
maxCiIterations: 5
---

# Step 5: PR & CI Loop

## STEP GOAL:

To update all story tracking artifacts, open a pull request, and iterate on CI failures and automated PR review feedback until the PR is green. If the loop cannot resolve after max iterations, skip the story and log the issue.

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

- 🎯 Focus on tracking updates, PR creation, and CI/review feedback resolution
- 🚫 FORBIDDEN to skip tracking updates before opening PR
- 🚫 FORBIDDEN to halt for user input
- 🔄 INNER LOOP: Open PR → wait for CI/review → fix feedback → push → re-wait until green or max iterations

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file with PR number
- 📖 Update story file and sprint status BEFORE opening PR
- 🚫 FORBIDDEN to exceed {maxCiIterations} iterations — skip and log if exceeded

## CONTEXT BOUNDARIES:

- Available: Story file, execution state, feature branch with all commits
- Focus: Tracking, PR, CI, automated review feedback
- Limits: Do not merge the PR — leave it open for human review
- Dependencies: step-04 must have completed code review

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Update Story File Tracking

Update the story file with final status:

- **Status:** Set to "review"
- **Dev Agent Record:** Ensure implementation notes are complete
- **Tasks/Subtasks:** Verify all marked [x]
- **File List:** Verify all new/modified files are listed
- **Change Log:** Ensure summary of changes is present

Commit these tracking updates.

### 2. Update Sprint Status and State File

**MANDATORY — hard requirement, not optional:**

Update {sprintStatusFile}:
- Read the FULL sprint-status.yaml file
- Set the current story's status to "review"
- Save the file, preserving ALL comments and structure
- Commit the sprint status update.

Update {stateFile}:
- Set `currentPhase` to "code-review" for the current story

### 2b. Pre-PR Local Validation Checklist

**For operational stories** (no code changes, only tracking file updates): Skip code-specific validations — proceed directly to step 3.

**For code stories — MANDATORY:** Before opening a PR, run all applicable local validations:
- `terraform validate` (for Terraform changes)
- `cargo clippy -- -D warnings` (for Rust changes)
- `cargo test` (for Rust changes)
- `cargo fmt --check` (for Rust changes)

Fix any failures before proceeding to step 3.

### 3. Push Branch and Open PR

Push the feature branch to remote:
```
git push -u origin {branch-name}
```

Open a pull request using `gh pr create`:
- **Title:** Story ID and title (e.g., "feat: 2.1 - implement DRR scheduler")
- **Body:** Summary of changes, acceptance criteria covered, test summary
- **Base:** Determine the PR target branch using this logic:
  - **If the story has dependencies** (i.e., a previous story in this epic): Target the dependent story's branch (e.g., Story 4.2 targets `feat/4.1-token-bucket-implementation`). This creates stacked/chained PRs with incremental diffs.
  - **If the story has no dependencies** (i.e., it is the first story in the epic): Target the epic's baseBranch (typically `main`).
  - **NEVER target `main` for stories that have dependencies.** This breaks Cubic review scope, cascade rebase, and tracking file consistency.

Record the PR number in {stateFile}.

### 4. Wait for CI and Automated PR Review (Inner Loop)

**LOOP START:**

Set iteration counter (start at 0, increment each loop).

**If iteration counter > {maxCiIterations}:**
- Log the issue in {stateFile} under skippedIssues:
  ```yaml
  - story: "{story-id}"
    phase: "pr-ci"
    reason: "CI/review loop exceeded max iterations ({maxCiIterations})"
    iteration: {current iteration}
  ```
- Set story status to "skipped" in {stateFile}
- Display: "**Story {story-id} skipped — CI/review loop could not resolve after {maxCiIterations} iterations.**"
- Proceed to step 5 below (auto-proceed to next step regardless)

**Check CI status:**
```
gh pr checks {pr-number} --watch
```

**Check for Cubic automated review (CRITICAL — has a delay after CI check):**
After all CI checks pass, specifically check the Cubic check output. The Cubic CI check summary shows
"AI review completed with N review(s). X issues found across Y files."
- If 0 issues found: Cubic has nothing to say — no review will be posted. Proceed.
- If >0 issues found: Cubic WILL post a review, but there is a **delay** after the check completes.
  Poll for the review to appear (check every 15-30 seconds, up to 2 minutes):
  ```
  gh api repos/{owner}/{repo}/pulls/{pr-number}/reviews
  ```
  Wait until Cubic's review appears before evaluating feedback.

**Check for all automated PR review comments:**
```
gh api repos/{owner}/{repo}/pulls/{pr-number}/reviews
gh api repos/{owner}/{repo}/pulls/{pr-number}/comments
```

**If CI passes AND no unresolved review feedback (including Cubic):**
- **Before exiting:** Verify every Cubic comment has a reply. List all Cubic comments:
  `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
  For each Cubic comment, confirm it has at least one non-Cubic reply (either "Addressed in [commit]" or "Not addressing: [reason]"). If any Cubic comment lacks a reply, reply to it now before proceeding.
- Exit loop
- Proceed to step 5

**If CI fails OR automated review has feedback:**
1. Read the failure details or review comments
2. Reply to EVERY Cubic review comment on the PR BEFORE pushing fixes:
   - For addressed comments: reply with "Addressed in [commit hash]"
   - For declined comments: reply with "Not addressing: [reason]"
   - List Cubic inline comments: `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
   - Reply to each: `gh api repos/{owner}/{repo}/pulls/comments/{comment_id}/replies -f body="..."`
3. Fix the issues in code
4. Commit and push
5. **Go back to LOOP START** (re-check CI and reviews)

### 4b. External SDK PR Lifecycle Gate

**If the story opened PRs in external repositories** (e.g., SDK repos like fila-go, fila-python, fila-js, fila-ruby, fila-java):

A story with external PRs is **NOT done** until every external PR has:
1. **CI passing** — check with `gh pr checks {pr-number} -R {org}/{repo}`
2. **Cubic review checked** — 0 issues found, or all findings addressed
3. **PR merged** — `gh pr merge {pr-number} -R {org}/{repo}` (or left for human review if configured)

For each external PR:
- Run the same CI + Cubic check loop as step 4 above
- If CI fails, launch a fix agent for that repo and re-check
- If Cubic has findings, address them and push fixes
- Do NOT proceed to step 5 until all external PRs are green

**Opening external PRs is not completing them.** This gate was added after Epic 30, where 5 SDK PRs were opened but not reviewed — all had CI failures and 11 Cubic findings.

### 5. Update State and Auto-Proceed

Update {stateFile}:
- Set currentPhase to "pr-complete"

Display: "**Story {story-id} PR #{pr-number} is green. Proceeding to completion...**"

#### Menu Handling Logic:

- After PR is green (or story is skipped), immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after PR is green or story is skipped

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Story file tracking fully updated before PR
- Sprint status updated
- PR opened with clear title and description
- CI passes
- Automated review feedback addressed
- PR number recorded in state file
- PR left open for human review (NOT merged)
- Auto-proceeded to step 06

### ❌ SYSTEM FAILURE:

- PR opened before tracking updates
- CI failures not addressed
- Automated review feedback ignored
- PR merged (should only be left open)
- State file not updated with PR number
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
