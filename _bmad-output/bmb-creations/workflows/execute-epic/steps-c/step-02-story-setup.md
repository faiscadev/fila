---
name: 'step-02-story-setup'
description: 'Create feature branch and generate detailed story spec for the current story'

nextStepFile: './step-03-story-dev.md'
stateFile: '{output_folder}/epic-execution-state.yaml'
createStoryWorkflow: '{project-root}/_bmad/bmm/workflows/4-implementation/create-story/workflow.yaml'
createStoryInstructions: '{project-root}/_bmad/bmm/workflows/4-implementation/create-story/instructions.xml'
createStoryTemplate: '{project-root}/_bmad/bmm/workflows/4-implementation/create-story/template.md'
createStoryChecklist: '{project-root}/_bmad/bmm/workflows/4-implementation/create-story/checklist.md'
---

# Step 2: Story Setup

## STEP GOAL:

To create a feature branch for the current story and generate its detailed story specification by executing the create-story process autonomously.

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

- 🎯 Focus ONLY on branch creation and story spec generation
- 🚫 FORBIDDEN to start implementing code in this step
- 🚫 FORBIDDEN to halt for user input
- 💬 Load create-story skill as reference and execute its process autonomously

## EXECUTION PROTOCOLS:

- 🎯 Follow the MANDATORY SEQUENCE exactly
- 💾 Update state file after branch creation and story spec generation
- 📖 Track progress in state file
- 🚫 FORBIDDEN to skip story spec validation

## CONTEXT BOUNDARIES:

- Available: Execution state file, epic plan, BMM config
- Focus: Branch creation and story spec only
- Limits: Do not implement code
- Dependencies: step-01 must have created the state file

## MANDATORY SEQUENCE

**CRITICAL:** Follow this sequence exactly. Do not skip, reorder, or improvise.

### 1. Read Current Story from State

Load {stateFile} and identify the current story to execute:

- Find the first story with status "pending" or the story with status "in-progress" and currentPhase "setup"
- Extract: story ID, title, dependencies, description

### 2. Update State to In-Progress

Update {stateFile}:
- Set current story status to "in-progress"
- Set currentPhase to "setup"

### 3. Create Feature Branch

Determine the base for this branch:

- **If the story has dependencies:** Check if the dependent story's branch exists and has an open PR. If so, branch from that story's branch. Otherwise, branch from the epic's baseBranch.
- **If the story has no dependencies:** Branch from the epic's baseBranch.

Create the branch:
```
git checkout -b feat/{story-id}-{short-description} {base}
```

Update {stateFile} with the branch name.

### 4. Load Create-Story Skill as Reference

Load {createStoryWorkflow}, {createStoryInstructions}, and {createStoryTemplate} as reference documents.

These define:
- How to analyze the epic plan and extract story requirements
- How to generate acceptance criteria with BDD format
- How to produce dev notes with technical guidance
- How to create the story file from the template
- How to validate the story spec quality

### 5. Execute Create-Story Process Autonomously

Following the create-story skill's process as reference:

1. Load the epic plan and extract the current story's full context
2. Load architecture docs, previous story files, and other relevant artifacts
3. Generate the detailed story specification:
   - Title, description, acceptance criteria (BDD format)
   - Tasks and subtasks
   - Dev notes with technical guidance, file patterns, dependencies
   - Implementation hints from previous stories
4. Write the story file using the template structure
5. Run the create-story validation checklist ({createStoryChecklist}) against the generated story
6. If validation finds issues, fix them and re-validate

**CRITICAL:** Do NOT invoke the create-story skill interactively. Load its documents as reference and execute the equivalent process directly, auto-proceeding through all steps without menus or user prompts.

### 6. Update State and Auto-Proceed

Update {stateFile}:
- Set currentPhase to "dev"

Display: "**Story {story-id} spec created. Proceeding to development...**"

#### Menu Handling Logic:

- After story spec is created and validated, immediately load, read entire file, then execute {nextStepFile}

#### EXECUTION RULES:

- This is an auto-proceed step with no user choices
- Proceed directly to next step after story setup

## 🚨 SYSTEM SUCCESS/FAILURE METRICS

### ✅ SUCCESS:

- Feature branch created from correct base
- Story spec generated with full BDD acceptance criteria
- Dev notes include technical guidance from architecture and previous stories
- Story spec validated against create-story checklist
- State file updated with branch name and phase
- Auto-proceeded to step 03

### ❌ SYSTEM FAILURE:

- Branch created from wrong base (ignoring dependencies)
- Story spec missing acceptance criteria or dev notes
- Validation skipped
- State file not updated
- Halting for user input

**Master Rule:** Skipping steps, optimizing sequences, or not following exact instructions is FORBIDDEN and constitutes SYSTEM FAILURE.
