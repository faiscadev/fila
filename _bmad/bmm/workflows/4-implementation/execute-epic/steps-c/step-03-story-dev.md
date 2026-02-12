---
name: 'step-03-story-dev'
description: 'Implement the current story by executing the dev-story process autonomously'

nextStepFile: './step-04-code-review.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
devStoryWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/dev-story/workflow.yaml'
devStoryInstructions: '{project-root}/_bmad/bmm/workflows/4-implementation/dev-story/instructions.xml'
devStoryChecklist: '{project-root}/_bmad/bmm/workflows/4-implementation/dev-story/checklist.md'
---

# Step 3: Story Development

## STEP GOAL:

To implement the current story — writing code, tests, and documentation — by executing the dev-story process autonomously.

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

- 🎯 Focus ONLY on implementing the story as specified
- 🚫 FORBIDDEN to skip writing tests
- 🚫 FORBIDDEN to halt for user input
- 💬 Load dev-story skill as reference and execute its process autonomously

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file after development is complete
- 📖 Update story file with implementation tracking (tasks, file list, dev agent record)
- 🚫 FORBIDDEN to mark tasks complete without actually implementing them

## CONTEXT BOUNDARIES:

- Available: Story spec file (created in step 02), execution state, codebase
- Focus: Code implementation, tests, story file updates
- Limits: Do not open PRs or perform code review
- Dependencies: step-02 must have created the story spec

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Load Story File

Read the story spec file created in step 02. Extract:
- Acceptance criteria
- Tasks and subtasks
- Dev notes with technical guidance
- File patterns and dependencies

### 2. Load Dev-Story Skill as Reference

Load {devStoryWorkflow}, {devStoryInstructions}, and {devStoryChecklist} as reference documents.

These define:
- How to implement tasks and subtasks systematically
- How to write tests that cover acceptance criteria
- How to update the story file with progress
- The definition of done checklist

### 3. Execute Dev-Story Process Autonomously

Following the dev-story skill's process as reference:

1. **Implement each task/subtask in order:**
   - Read the task requirements
   - Write the implementation code
   - Mark the task as complete [x] in the story file
   - Commit the work with a descriptive message

2. **Write tests for every acceptance criterion:**
   - Unit tests for core functionality
   - Integration tests where specified
   - Ensure all existing tests still pass

3. **Update story file tracking:**
   - Mark all tasks/subtasks as complete
   - Update the File List with all new/modified files
   - Update the Dev Agent Record with implementation notes
   - Update the Change Log

4. **Run the definition of done checklist ({devStoryChecklist}):**
   - Verify all tasks complete
   - Verify all acceptance criteria satisfied
   - Verify tests pass
   - Verify documentation updated

**CRITICAL:** Do NOT invoke the dev-story skill interactively. Load its documents as reference and execute the equivalent process directly, auto-proceeding through all steps without menus or user prompts.

### 4. Ensure All Tests Pass

Run the project's test suite. If any tests fail:
- Fix the failing tests
- Re-run until all tests pass
- Do not proceed with failing tests

### 5. Commit All Work

Ensure all implementation work is committed to the feature branch with clear, descriptive commit messages following project conventions.

### 6. Update State and Auto-Proceed

Update {stateFile}:
- Set currentPhase to "code-review"

Display: "**Story {story-id} implemented. Proceeding to code review...**"

#### Menu Handling Logic:

- After development is complete and all tests pass, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after development

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- All tasks and subtasks implemented and marked complete
- Tests written for every acceptance criterion
- All tests passing (no regressions)
- Story file updated: tasks, file list, dev agent record, change log
- All work committed to feature branch
- Definition of done checklist passes
- State file updated with phase
- Auto-proceeded to step 04

### ❌ SYSTEM FAILURE:

- Tasks marked complete without implementation
- Missing tests for acceptance criteria
- Failing tests not fixed
- Story file tracking not updated
- Uncommitted work
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
