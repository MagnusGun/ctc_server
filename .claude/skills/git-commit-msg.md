# Git Commit Message Generator

This skill generates concise, best-practice git commit messages following the 50-character limit.

## When to Use

Use this skill when the user asks for:
- A git commit message
- A commit msg
- "git commit"
- How to commit their changes

## Guidelines

**Format:**
```
<verb> <what> [context if space allows]
```

**Rules:**
1. **Maximum 50 characters** (hard limit)
2. **Start with imperative verb**: Add, Fix, Update, Remove, Refactor, Improve
3. **No period at end**
4. **Be specific but concise**
5. **Focus on WHAT, not HOW**

**Common Verbs:**
- `Add` - New feature/file
- `Fix` - Bug fix
- `Update` - Modify existing
- `Remove` - Delete code/feature
- `Refactor` - Restructure without changing behavior
- `Improve` - Enhance existing functionality
- `Implement` - Complete a planned feature

## Process

1. **NEVER run `git add` or `git commit`** - this skill only generates the command

2. **ALWAYS check staged changes first** (this is what will be committed):
   ```bash
   git diff --cached --stat
   git diff --cached --name-only
   ```

3. **If no staged changes**, check unstaged changes:
   ```bash
   git status
   git diff --stat
   ```

4. **Analyze changes** to identify the primary purpose

5. **Generate message** following format:
   - Count characters (max 50)
   - Use imperative verb
   - Be specific about what changed
   - Omit implementation details

6. **Output the full git commit command** in this format:
   ```bash
   git commit -m "Your message here"
   ```

## Important: Staged vs Unstaged

- **Staged changes** (`git diff --cached`) are what WILL be committed
- **Unstaged changes** (`git diff`) are NOT yet staged for commit
- **ALWAYS prioritize staged changes** - that's what the user is committing
- If no staged changes exist, warn the user they need to `git add` files first

## Examples

**Good (under 50 chars):**
```bash
git commit -m "Add custom error types with unit tests"
git commit -m "Fix step validation bug in actor"
git commit -m "Update Docker config for ARM64 build"
git commit -m "Remove unused helper functions"
git commit -m "Refactor temperature routes to use helpers"
git commit -m "Implement timeout handling for Modbus ops"
```

**Bad (too long or poor format):**
```
Added a new custom error type system with ModbusError and ApiError (62 chars - TOO LONG)
fixed bug (not imperative, not specific)
Update some files (too vague)
Implemented the new feature that allows users to... (too long)
```

## Important Notes

- If changes are too complex for one message, suggest breaking into multiple commits
- Never include newlines in the message (single line only)
- Don't add emoji unless user specifically requests it
- Don't add "Co-Authored-By" or other trailers (user can add those separately)
