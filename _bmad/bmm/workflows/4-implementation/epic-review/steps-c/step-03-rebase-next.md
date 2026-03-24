---
name: 'step-03-rebase-next'
description: 'Rebase the next PR branch onto the updated parent after a merge, with enforced conflict reasoning and CI/Cubic gate'

nextStepFile: './step-02-review-pr.md'
stateFile: '{implementation_artifacts}/epic-review-state.yaml'
maxCiIterations: 5
---

# Step 3: Rebase Next PR

## STEP GOAL:

To rebase the next story's branch onto the updated parent branch (which was just merged), resolve any conflicts with careful reasoning about the correct merged state, push, pass the CI/Cubic gate, and then return to step-02 for the next PR review.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are a disciplined review-cycle facilitator
- ✅ Conflict resolution requires ACTIVE REASONING, not mechanical auto-resolution
- ✅ You think carefully about what the correct merged state should be
- ✅ CI and Cubic gates are non-negotiable

### Step-Specific Rules:

- 🎯 Focus ONLY on rebasing, conflict resolution, and CI/Cubic gate
- 🚫 FORBIDDEN to auto-resolve conflicts without reasoning about correct state
- 🚫 FORBIDDEN to skip CI/Cubic gate after pushing
- 🚫 FORBIDDEN to halt for user input — this step is autonomous
- ⚠️ CONFLICT RESOLUTION IS THE MOST CRITICAL PART — see the Conflict Resolution Protocol below

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file after rebase and CI/Cubic gate
- 📖 Document conflict resolution reasoning in commit messages
- 🚫 FORBIDDEN to use `git checkout --theirs` or `git checkout --ours` without per-file reasoning

## CONTEXT BOUNDARIES:

- Available: Review state file, git branches, GitHub PR data
- Focus: Rebase, conflict resolution, CI/Cubic enforcement
- Limits: Do not review PRs — that's step-02
- Dependencies: A PR was just merged in step-02

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load State and Identify Branches

Read {stateFile} to get:
- The branch that was just merged (previous PR's branch) — call this `parent-branch`
- The next PR's branch — call this `next-branch`
- The next PR's base branch (should be `parent-branch`)

### 2. Fetch Updated Branches

```
git fetch origin
git checkout {next-branch}
git pull origin {next-branch}
```

Identify the updated parent. After the previous PR was merged:
- If the parent was the first PR (base = main): `git fetch origin main`
- Otherwise: the parent branch was merged into its own parent, so the target is now the merged-into branch. Determine the correct upstream ref.

**The rebase target is the branch that the next PR targets** (its baseBranch). After the previous merge, this branch is updated. Fetch it:
```
git fetch origin {baseBranch}
```

### 3. Rebase

```
git rebase origin/{baseBranch}
```

**If rebase completes cleanly (no conflicts):** Skip to step 5.

**If conflicts occur:** Proceed to step 4 (Conflict Resolution Protocol).

### 4. Conflict Resolution Protocol

**🚨 THIS IS THE MOST CRITICAL SECTION OF THIS WORKFLOW.**

The recurring problem across 4 epics: the agent resolves conflicts mechanically without thinking about what the correct merged state should be. Files like sprint-status.yaml, story artifacts, and tracking files have BOTH sides partially correct. Blindly picking one side corrupts state.

**FOR EACH CONFLICTING FILE, you MUST follow this protocol:**

#### 4a. Read Both Sides

For each conflicting file:
```
git diff --name-only --diff-filter=U
```

For each file with conflicts, read the full file content showing conflict markers.

#### 4b. Understand Intent

Ask yourself these questions and answer them explicitly in your reasoning:

1. **What did the parent branch change in this file, and why?**
   - Read the parent's version. What state does it represent?

2. **What did the current branch change in this file, and why?**
   - Read the current branch's version. What state does it represent?

3. **What is the CORRECT merged state?**
   - This is NOT always "pick one side." Often both sides made valid changes to different parts.
   - For sprint-status.yaml: both sides may have updated different story statuses. The correct state includes ALL status updates from both sides.
   - For story artifacts: the parent may have updated status to "done" while the current branch added new content. Both changes should be preserved.
   - For code files: understand what each side's changes accomplish. Do they conflict logically, or just textually?

#### 4c. Resolve with Explanation

Resolve the conflict by constructing the correct merged state. Then explain your reasoning:

"**Conflict in {filename}:**
- Parent changed: {what and why}
- Current branch changed: {what and why}
- Correct resolution: {what the merged state should be and why}"

#### 4d. Mark Resolved and Continue

```
git add {resolved-file}
```

After ALL conflicts are resolved:
```
git rebase --continue
```

If new conflicts appear during continued rebase, repeat the protocol for each one.

#### 4e. Document in Commit

After rebase completes, the rebase commit messages are preserved. No additional commit needed for conflict resolution — but if you had to make resolution choices, ensure the rebase commit messages are descriptive.

### 5. Force Push

The branch has been rebased, so a force push is required:
```
git push --force-with-lease origin {next-branch}
```

### 6. CI/Cubic Gate

**CRITICAL: This gate is non-negotiable. Follow every sub-step.**

Set iteration counter to 0.

**GATE LOOP START:**

Increment iteration counter. **If counter > {maxCiIterations}:** Report "CI/Cubic gate could not be resolved after {maxCiIterations} iterations." Display the issue summary and return to step-02 to present the PR to the user for guidance.

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
2. Reply to EVERY Cubic review comment on the PR BEFORE pushing fixes:
   - For addressed comments: reply with "Addressed in [commit hash]"
   - For declined comments: reply with "Not addressing: [reason]"
   - List Cubic inline comments: `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
   - Reply to each: `gh api repos/{owner}/{repo}/pulls/comments/{comment_id}/replies -f body="..."`
3. Fix the issues in code
4. Add regression tests for Cubic findings where appropriate
5. Commit: `fix: address CI/Cubic findings for story {storyId} after rebase`
6. Push: `git push origin {next-branch}`
7. **Go back to GATE LOOP START**

**If CI passes AND no Cubic findings (or 0 issues):**
- **Before exiting:** Verify every Cubic comment has a reply. List all Cubic comments:
  `gh api repos/{owner}/{repo}/pulls/{pr-number}/comments --jq '[.[] | select(.user.login == "cubic-dev-ai[bot]")]'`
  For each Cubic comment, confirm it has at least one non-Cubic reply (either "Addressed in [commit]" or "Not addressing: [reason]"). If any Cubic comment lacks a reply, reply to it now before proceeding.
- Gate passes. Exit loop.

### 7. Update State and Proceed

Update {stateFile}:
- Confirm currentPrIndex points to the next PR
- Status is "pending" (ready for user review)

Display: "**Rebase complete. CI/Cubic gate passed. Proceeding to PR review...**"

#### Menu Handling Logic:

- After CI/Cubic gate passes, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step — no user choices
- Proceed directly to step-02 after rebase and gate pass

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Next branch rebased onto updated parent
- All conflicts resolved with explicit reasoning about correct merged state
- Conflict resolution reasoning documented
- Force push successful
- CI/Cubic gate passed
- State file updated
- Auto-proceeded to step-02

### ❌ SYSTEM FAILURE:

- Auto-resolving conflicts without reasoning (using --theirs/--ours blindly)
- Not reading both sides of a conflict before resolving
- Skipping CI/Cubic gate after push
- Not documenting conflict resolution reasoning
- Halting for user input during rebase

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE. The conflict resolution protocol is the CORE VALUE of this step — never shortcut it.
