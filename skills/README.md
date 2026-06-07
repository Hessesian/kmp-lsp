# skills — kotlin-lsp project skills

This directory contains agent skills for kotlin-lsp. Skills teach AI coding agents
(e.g., Claude Code, Cursor, Copilot, pi) how to interact with kotlin-lsp
efficiently — when to call which command, how to parse output, and what pitfalls
to avoid.

## For consumers — installing the skill in your project

If you use kotlin-lsp in your own project and want your AI agent to prefer it
over raw `grep` / `rg`, install the skill:

```bash
npx skills add https://github.com/qdsfdhvh/kotlin-lsp
```

This copies `skills/kotlin-lsp/SKILL.md` into your project's agent directory
(e.g., `CLAUDE.md`, `.cursor/rules/`, or `pi/`). After installation, your agent
will know how to use `kotlin-lsp find`, `refs`, `hover`, `check`, etc.

To update after a kotlin-lsp release:

```bash
npx skills update kotlin-lsp
```

## For contributors — skill layout

```
skills/
├── README.md              ← this file: skill overview & usage
└── kotlin-lsp/
    └── SKILL.md            ← the main skill consumed by downstream projects
```

## How skills work

A skill is a `SKILL.md` file with YAML frontmatter (`name`, `description`). When
installed via `npx skills add <url>`, the `npx skills` tool:

1. Clones/fetches the repo
2. Reads `skills/<name>/SKILL.md`
3. Copies it into the target project's agent config directory

The frontmatter `description` is indexed for search. The body teaches the agent
what commands are available and when to use them.

## How to add a new skill

```bash
npx skills init <name>         # creates skills/<name>/SKILL.md
# or: manually create skills/<name>/SKILL.md with frontmatter
```

Push the new skill file to the repo. Downstream users run `npx skills update`
to pick it up.

## Best practices for skill content

- Keep the frontmatter `name` concise and `description` scannable (it's what
  shows in `npx skills list`)
- Include a decision tree or "when to use" section so agents can self-select
- Prefer tables over prose for command references
- Show concrete examples for each command
- Document output format choices (`--json`, `--flat`, `--relative`)
- Call out anti-patterns explicitly
