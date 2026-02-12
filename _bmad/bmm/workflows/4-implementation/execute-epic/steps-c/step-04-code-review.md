---
name: 'step-04-code-review'
description: 'Run code review loop until clean, referencing the code-review skill'

nextStepFile: './step-05-pr-ci.md'
skipStepFile: './step-06-story-complete.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
codeReviewWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/code-review/workflow.yaml'
codeReviewInstructions: '{project-root}/_bmad/bmm/workflows/4-implementation/code-review/instructions.xml'
codeReviewChecklist: '{project-root}/_bmad/bmm/workflows/4-implementation/code-review/checklist.md'
maxReviewIterations: 5
---

# Step 4: Code Review Loop

## STEP GOAL:

To perform adversarial code review on the implemented story, iterating fixes until the review is clean. If the loop cannot resolve after max iterations, skip the story and log the issue.

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

- 🎯 Focus ONLY on code review and fixing issues found
- 🚫 FORBIDDEN to approve without thorough review
- 🚫 FORBIDDEN to halt for user input
- 💬 Load code-review skill as reference and execute its process autonomously
- 🔄 INNER LOOP: Review → fix → re-review until clean or max iterations

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file after review is complete
- 📖 Track review iterations
- 🚫 FORBIDDEN to exceed {maxReviewIterations} iterations — skip and log if exceeded

## CONTEXT BOUNDARIES:

- Available: Story spec, implemented code, execution state
- Focus: Code quality, test coverage, architecture compliance
- Limits: Do not open PRs
- Dependencies: step-03 must have completed development

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load Code Review Skill as Reference

Load {codeReviewWorkflow}, {codeReviewInstructions}, and {codeReviewChecklist} as reference documents.

These define:
- How to perform adversarial senior developer code review
- What to check: code quality, test coverage, architecture compliance, security, performance
- How to verify every acceptance criterion has a corresponding test
- The review checklist

### 2. Initialize Review Loop

Set iteration counter to 0.

### 3. Execute Code Review (Inner Loop)

**LOOP START:**

Increment iteration counter.

**If iteration counter > {maxReviewIterations}:**
- Log the issue in {stateFile} under skippedIssues:
  ```yaml
  - story: "{story-id}"
    phase: "code-review"
    reason: "Review loop exceeded max iterations ({maxReviewIterations})"
    iteration: {current iteration}
  ```
- Set story status to "skipped" in {stateFile}
- Display: "**Story {story-id} skipped — review loop could not resolve after {maxReviewIterations} iterations.**"
- Load, read entirely, then execute {skipStepFile}

**Execute review following the code-review skill's process:**

1. Load the story file and all changed files
2. Cross-check acceptance criteria against implementation
3. Verify every AC has a corresponding test
4. Review code quality: naming, structure, error handling, security
5. Check architecture compliance
6. Identify specific issues with clear descriptions

**If review finds issues:**
- Fix each issue identified
- Ensure all tests still pass after fixes
- Commit fixes
- **Go back to LOOP START** (re-review)

**If review is clean (no issues found):**
- Exit loop
- Proceed to step 4

**CRITICAL:** Do NOT invoke the code-review skill interactively. Load its documents as reference and execute the equivalent process directly, auto-proceeding through all steps without menus or user prompts.

### 4. Update State and Auto-Proceed

Update {stateFile}:
- Set currentPhase to "pr-ci"

Display: "**Story {story-id} code review passed. Proceeding to PR...**"

#### Menu Handling Logic:

- After review is clean, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after review passes

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Code review executed thoroughly using skill reference
- Every acceptance criterion verified to have a test
- All issues found and fixed
- All tests passing after fixes
- Review loop resolved within max iterations
- State file updated
- Auto-proceeded to step 05

### ❌ SYSTEM FAILURE:

- Rubber-stamp approval without thorough review
- Missing test coverage for acceptance criteria not caught
- Fixes not committed
- Infinite loop (no max iteration guard)
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
