---
name: 'step-01-init'
description: 'Discover the stacked PR chain from sprint-status and story artifacts, initialize review state'

continueFile: './step-01b-continue.md'
nextStepFile: './step-02-review-pr.md'
stateFile: '{implementation_artifacts}/epic-review-state.yaml'
sprintStatusFile: '{implementation_artifacts}/sprint-status.yaml'
executeEpicStateFile: '{output_folder}/epic-execution-state.yaml'
---

# Step 1: Initialize Epic Review

## STEP GOAL:

To discover the stacked PR chain for the target epic by reading sprint-status, the execute-epic state file, and story artifact files — then initialize the review state file and present the chain overview.

## MANDATORY EXECUTION RULES (READ FIRST):

### Universal Rules:

- 📖 CRITICAL: Read the complete step file before taking any action
- 🔄 CRITICAL: When loading next step, ensure entire file is read
- 🎯 ALWAYS follow the exact instructions in the step file
- ⚙️ TOOL/SUBPROCESS FALLBACK: If any instruction references a subprocess, subagent, or tool you do not have access to, you MUST still achieve the outcome in your main context thread

### Role Reinforcement:

- ✅ You are a disciplined review-cycle facilitator
- ✅ You discover the PR chain autonomously from project state
- ✅ Zero tolerance for skipped steps or incomplete tracking

### Step-Specific Rules:

- 🎯 Focus ONLY on discovering the PR chain and initializing state
- 🚫 FORBIDDEN to start reviewing PRs in this step
- 🚫 FORBIDDEN to skip reading story artifact files
- 💬 Present the discovered chain to the user for confirmation

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Create the review state file with all PRs
- 📖 Track initialization in the state file

## CONTEXT BOUNDARIES:

- Available: BMM config, sprint-status, execute-epic state, story artifact files
- Focus: PR chain discovery and state initialization
- Limits: Do not begin PR review
- Dependencies: None — this is the first step

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Check for Existing Review State

Look for an existing review state file at {stateFile}.

- **If the file exists and has PRs with status other than "pending":** A previous review session was interrupted. Load, read entirely, then execute {continueFile} to resume.
- **If the file does not exist or all PRs are "pending":** Continue to step 2.

### 2. Identify Target Epic

Read {sprintStatusFile} and identify which epic to review:

- Look for epics with status "in-progress" or "done" that have stories in "review" status
- If exactly one epic matches: auto-select it
- If multiple epics match: present the options and ask the user which epic to review
- If no epics match: report "No epics found with stories in review status" and halt

### 3. Read Execute-Epic State File

Read {executeEpicStateFile} to extract for each story in the target epic:

- Story ID
- Branch name
- PR number
- Status (should be "pr-complete" or similar)
- Execution order (the order stories appear in the state file IS the correct chain order)

**If the execute-epic state file does not exist or lacks PR info:** Fall back to discovering branches and PRs from git:
- List branches matching the epic pattern: `git branch -r --list 'origin/feat/{epic-number}*'`
- For each branch, find its PR: `gh pr list --head {branch-name} --json number,title,baseRefName`
- Order by story number

### 4. Read Story Artifact Files

For each story in the chain, read its artifact file from {implementation_artifacts}:

- Verify the story status is "review" (or "done" if already merged in a previous partial run)
- Extract the story title for display

### 5. Build the PR Chain

Assemble the ordered PR chain. Each entry needs:

```yaml
- storyId: "X.Y"
  storyKey: "X-Y-story-name"
  title: "Story title"
  branch: "feat/X.Y-branch-name"
  pr: 42
  baseBranch: "main"  # or previous story's branch
  status: pending      # pending, reviewing, merged
```

**Chain order rule:** The first story's PR targets main. Each subsequent story's PR targets the previous story's branch. Verify this matches the actual PR base branches on GitHub using `gh pr view {pr-number} --json baseRefName`.

### 6. Create Review State File

Create {stateFile} with the following structure:

```yaml
epic: "{epic-name}"
epicNumber: {N}
startedAt: "{current date/time}"
currentPrIndex: 0
prChain:
  - storyId: "X.1"
    storyKey: "X-1-story-name"
    title: "Story title"
    branch: "feat/X.1-branch-name"
    pr: 42
    baseBranch: "main"
    status: pending
  - storyId: "X.2"
    storyKey: "X-2-story-name"
    title: "Story title"
    branch: "feat/X.2-branch-name"
    pr: 43
    baseBranch: "feat/X.1-branch-name"
    status: pending
  # ... repeat for all stories
```

### 7. Present Chain Overview

Display the PR chain to the user:

"**Epic Review: {epic-name}**

PR chain discovered ({N} PRs):

| # | Story | PR | Branch | Base |
|---|-------|----|--------|------|
| 1 | {title} | #{pr} | {branch} | {base} |
| 2 | {title} | #{pr} | {branch} | {base} |
| ... | | | | |

**Proceeding to first PR...**"

### 8. Auto-Proceed to Review

#### Menu Handling Logic:

- After chain overview is presented, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices at this point
- Proceed directly to step-02 after presenting the chain

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- PR chain discovered from sprint-status + execute-epic state + story artifacts
- All PRs verified to exist on GitHub
- Chain order matches the stacked PR structure
- Review state file created with all PRs as "pending"
- Chain overview presented to user
- Auto-proceeded to step-02

### ❌ SYSTEM FAILURE:

- PR chain not discovered or incomplete
- State file not created
- Chain order incorrect (PRs not targeting previous story's branch)
- Skipping story artifact file reads

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
